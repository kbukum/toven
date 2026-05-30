//! Cache decision planning and persistent record storage.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use rskit_cache::CacheStore;

use crate::{
    cache::{
        input::{compute_shared_inputs_hash, compute_source_hashes},
        key::{CacheKey, CacheKeyBuilder},
        store::{FileCache, FileCacheConfig},
    },
    core::{
        AppError, AppResult, CommandOrigin, ErrorCode, ExecutionMode, ExecutionUnit, Module,
        ModuleId, Plan, Workspace,
    },
    exec::{render_execution_unit, render_resource_group},
};

/// Cache schema version for records and key composition.
pub const CACHE_RECORD_SCHEMA: u16 = 2;
/// Cache store path segment for the current record/key schema.
pub const CACHE_DIRECTORY: &str = "v2";

/// Effective cache mode for a command invocation.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum CacheMode {
    /// Read cache records and write successful executions.
    ReadWrite,
    /// Skip reads but write successful executions.
    Force,
    /// Do not read or write cache records.
    Disabled {
        /// Human-readable disable reason.
        reason: String,
    },
}

/// Cache decision for one module/task/profile tuple.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CacheDecision {
    /// Profile that owns the task.
    pub profile: String,
    /// Module covered by the decision.
    pub module: ModuleId,
    /// Task name.
    pub task: String,
    /// Final cache key.
    pub key: CacheKey,
    /// Source input hash.
    pub source_hash: String,
    /// Dependency input hash.
    pub dep_hash: String,
    /// Task definition hash.
    pub task_hash: String,
    /// Shared input hash.
    pub shared_hash: String,
    /// Lookup state.
    pub state: CacheState,
}

/// Cache lookup state.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum CacheState {
    /// Matching record exists.
    Hit,
    /// No matching usable record exists.
    Miss {
        /// Human-readable miss reason.
        reason: String,
    },
    /// Cache lookup and writes are disabled.
    Disabled {
        /// Human-readable disable reason.
        reason: String,
    },
    /// Cache read was skipped due to force mode.
    Forced,
}

impl CacheDecision {
    /// Return whether this module is satisfied by cache.
    #[must_use]
    pub fn is_hit(&self) -> bool {
        self.state == CacheState::Hit
    }

    /// Return whether successful execution should persist a record.
    #[must_use]
    pub const fn should_write(&self) -> bool {
        matches!(self.state, CacheState::Miss { .. } | CacheState::Forced)
    }
}

/// Prepared cache decisions keyed by `(profile, module)`.
pub type CacheDecisions = BTreeMap<(String, ModuleId), CacheDecision>;

/// Filesystem-backed task cache.
pub struct TaskCache {
    runtime: tokio::runtime::Runtime,
    store: FileCache,
}

impl TaskCache {
    /// Create a task cache rooted under the workspace.
    pub fn new(root: PathBuf) -> AppResult<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| {
                AppError::new(ErrorCode::Internal, "failed to create cache runtime")
                    .with_cause(error)
            })?;
        Ok(Self {
            runtime,
            store: FileCache::new(FileCacheConfig::new(root)),
        })
    }

    fn get_record(&self, key: &CacheKey) -> AppResult<Option<CacheRecord>> {
        let Some(value) = self.runtime.block_on(self.store.get(key.as_str()))? else {
            return Ok(None);
        };
        if let Ok(record) = serde_json::from_str::<CacheRecord>(&value) {
            Ok(Some(record))
        } else {
            self.runtime.block_on(self.store.delete(key.as_str()))?;
            Ok(None)
        }
    }

    /// Store a successful execution record.
    pub fn write_success(&self, decision: &CacheDecision, argv: &[String]) -> AppResult<()> {
        if !decision.should_write() {
            return Ok(());
        }
        let record = CacheRecord {
            schema: CACHE_RECORD_SCHEMA,
            key: decision.key.as_str().to_string(),
            profile: decision.profile.clone(),
            module: decision.module.to_string(),
            task: decision.task.clone(),
            source_hash: decision.source_hash.clone(),
            dep_hash: decision.dep_hash.clone(),
            task_hash: decision.task_hash.clone(),
            shared_hash: decision.shared_hash.clone(),
            argv: argv.to_vec(),
            success: true,
            created_at_epoch_secs: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|error| {
                    AppError::new(ErrorCode::Internal, "system clock is before UNIX epoch")
                        .with_cause(error)
                })?
                .as_secs(),
        };
        let value = serde_json::to_string(&record).map_err(|error| {
            AppError::new(ErrorCode::Internal, "failed to encode cache record").with_cause(error)
        })?;
        self.runtime
            .block_on(self.store.set(decision.key.as_str(), &value, None))
    }
}

/// Persistent record stored as a cache value.
#[derive(Debug, Clone, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct CacheRecord {
    /// Record schema version.
    pub schema: u16,
    /// Final cache key.
    pub key: String,
    /// Profile name.
    pub profile: String,
    /// Module id.
    pub module: String,
    /// Task name.
    pub task: String,
    /// Source input hash.
    pub source_hash: String,
    /// Dependency input hash.
    pub dep_hash: String,
    /// Task definition hash.
    pub task_hash: String,
    /// Shared input hash.
    pub shared_hash: String,
    /// Rendered argv that produced the record.
    pub argv: Vec<String>,
    /// Whether the command completed successfully.
    pub success: bool,
    /// Creation time in epoch seconds.
    pub created_at_epoch_secs: u64,
}

/// Prepare cache decisions for all modules covered by `full_plan`.
pub fn prepare_cache_decisions(
    full_plan: &Plan,
    mode: &CacheMode,
    task_cache: Option<&TaskCache>,
) -> AppResult<CacheDecisions> {
    if let CacheMode::Disabled { reason } = mode {
        return Ok(disabled_decisions(full_plan, reason));
    }
    let planned = planned_modules(full_plan);
    let effective_modes = planned
        .iter()
        .map(|(key, planned_module)| (key.clone(), effective_mode(mode, planned_module.unit)))
        .collect::<BTreeMap<_, _>>();
    let mut cache_keys = BTreeMap::new();
    let mut decisions = BTreeMap::new();

    if effective_modes
        .values()
        .all(|mode| matches!(mode, CacheMode::Disabled { .. }))
    {
        for (key, planned_module) in &planned {
            let CacheMode::Disabled { reason } = &effective_modes[key] else {
                unreachable!("all cache modes are disabled");
            };
            decisions.insert(
                (key.0.clone(), key.1.clone()),
                disabled_decision(planned_module.unit, planned_module.module, reason),
            );
        }
        return Ok(decisions);
    }

    let all_modules = modules_from_plan(full_plan)?;
    let source_hashes = compute_source_hashes(&full_plan.workspace, &all_modules)?;

    for key in planned.keys() {
        let planned_module = planned.get(key).ok_or_else(|| {
            AppError::invalid_input("modules", format!("module '{}' is not planned", key.1))
        })?;
        let effective_mode = effective_modes
            .get(key)
            .cloned()
            .expect("effective mode exists for every planned key");
        if let CacheMode::Disabled { reason } = &effective_mode {
            decisions.insert(
                (key.0.clone(), key.1.clone()),
                disabled_decision(planned_module.unit, planned_module.module, reason),
            );
            continue;
        }
        let components = compute_components(
            key,
            &planned,
            &full_plan.workspace,
            &source_hashes.modules,
            &mut cache_keys,
            &mut BTreeSet::new(),
        )?;
        let state = lookup_state(&effective_mode, task_cache, &components)?;
        decisions.insert(
            (key.0.clone(), key.1.clone()),
            CacheDecision {
                profile: key.0.clone(),
                module: key.1.clone(),
                task: components.task.clone(),
                key: components.key,
                source_hash: components.source_hash,
                dep_hash: components.dep_hash,
                task_hash: components.task_hash,
                shared_hash: components.shared_hash,
                state,
            },
        );
    }

    Ok(decisions)
}

fn disabled_decisions(plan: &Plan, reason: &str) -> CacheDecisions {
    let mut decisions = BTreeMap::new();
    for unit in &plan.units {
        for module in &unit.modules {
            decisions.insert(
                (unit.profile.clone(), module.name.clone()),
                disabled_decision(unit, module, reason),
            );
        }
    }
    decisions
}

fn disabled_decision(unit: &ExecutionUnit, module: &Module, reason: &str) -> CacheDecision {
    CacheDecision {
        profile: unit.profile.clone(),
        module: module.name.clone(),
        task: unit.task.clone(),
        key: CacheKey::new("disabled"),
        source_hash: "disabled".to_string(),
        dep_hash: "disabled".to_string(),
        task_hash: "disabled".to_string(),
        shared_hash: "disabled".to_string(),
        state: CacheState::Disabled {
            reason: reason.to_string(),
        },
    }
}

fn lookup_state(
    mode: &CacheMode,
    task_cache: Option<&TaskCache>,
    components: &KeyComponents,
) -> AppResult<CacheState> {
    match mode {
        CacheMode::Disabled { reason } => Ok(CacheState::Disabled {
            reason: reason.clone(),
        }),
        CacheMode::Force => Ok(CacheState::Forced),
        CacheMode::ReadWrite => {
            let Some(cache) = task_cache else {
                return Ok(CacheState::Disabled {
                    reason: "cache store unavailable".to_string(),
                });
            };
            match cache.get_record(&components.key)? {
                Some(record) if record_matches(&record, components) => Ok(CacheState::Hit),
                Some(_) => Ok(CacheState::Miss {
                    reason: "cache record did not match current inputs".to_string(),
                }),
                None => Ok(CacheState::Miss {
                    reason: "no cache record".to_string(),
                }),
            }
        }
    }
}

fn effective_mode(mode: &CacheMode, unit: &ExecutionUnit) -> CacheMode {
    if unit.persistent {
        return CacheMode::Disabled {
            reason: "persistent tasks are never cached".to_string(),
        };
    }
    if matches!(mode, CacheMode::ReadWrite | CacheMode::Force)
        && !unit.passthrough_args.is_empty()
        && !unit.cache_args
    {
        return CacheMode::Disabled {
            reason: "passthrough args disable cache".to_string(),
        };
    }
    mode.clone()
}

fn record_matches(record: &CacheRecord, components: &KeyComponents) -> bool {
    record.schema == CACHE_RECORD_SCHEMA
        && record.success
        && record.key == components.key.as_str()
        && record.profile == components.profile
        && record.module == components.module.to_string()
        && record.task == components.task
        && record.source_hash == components.source_hash
        && record.dep_hash == components.dep_hash
        && record.task_hash == components.task_hash
        && record.shared_hash == components.shared_hash
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct PlannedKey(String, ModuleId);

#[derive(Debug, Clone)]
struct PlannedModule<'a> {
    unit: &'a ExecutionUnit,
    module: &'a Module,
}

#[derive(Debug, Clone)]
struct KeyComponents {
    profile: String,
    module: ModuleId,
    task: String,
    key: CacheKey,
    source_hash: String,
    dep_hash: String,
    task_hash: String,
    shared_hash: String,
}

fn compute_components(
    key: &PlannedKey,
    planned: &BTreeMap<PlannedKey, PlannedModule<'_>>,
    workspace: &Workspace,
    source_hashes: &BTreeMap<ModuleId, String>,
    cache_keys: &mut BTreeMap<PlannedKey, KeyComponents>,
    visiting: &mut BTreeSet<PlannedKey>,
) -> AppResult<KeyComponents> {
    if let Some(components) = cache_keys.get(key) {
        return Ok(components.clone());
    }
    if !visiting.insert(key.clone()) {
        return Err(AppError::invalid_input(
            "modules",
            format!("module dependency cycle includes '{}'", key.1),
        ));
    }
    let planned_module = planned.get(key).ok_or_else(|| {
        AppError::invalid_input("modules", format!("module '{}' is not planned", key.1))
    })?;
    let dep_keys = planned_module
        .module
        .dependencies
        .iter()
        .map(|dependency| {
            let dep_key = PlannedKey(key.0.clone(), dependency.clone());
            if planned.contains_key(&dep_key) {
                compute_components(
                    &dep_key,
                    planned,
                    workspace,
                    source_hashes,
                    cache_keys,
                    visiting,
                )
                .map(|components| components.key.as_str().to_string())
            } else {
                Ok(source_hashes
                    .get(dependency)
                    .cloned()
                    .unwrap_or_else(|| format!("external:{dependency}")))
            }
        })
        .collect::<AppResult<Vec<_>>>()?;

    visiting.remove(key);

    let source_hash = source_hashes.get(&key.1).cloned().ok_or_else(|| {
        AppError::invalid_input("modules", format!("module '{}' has no source hash", key.1))
    })?;
    let dep_hash = component_hash("deps", dep_keys);
    let shared_hash = compute_shared_inputs_hash(workspace, &planned_module.unit.shared_inputs)?;
    let task_hash = task_hash(
        planned_module.unit,
        planned_module.module,
        workspace,
        &shared_hash,
    )?;
    let cache_key = CacheKeyBuilder::new()
        .field(&key.0)
        .field(key.1.as_str())
        .field(&planned_module.unit.task)
        .field(&source_hash)
        .field(&dep_hash)
        .field(&task_hash)
        .field(&shared_hash)
        .build();
    let components = KeyComponents {
        profile: key.0.clone(),
        module: key.1.clone(),
        task: planned_module.unit.task.clone(),
        key: cache_key,
        source_hash,
        dep_hash,
        task_hash,
        shared_hash,
    };
    cache_keys.insert(key.clone(), components.clone());
    Ok(components)
}

fn task_hash(
    unit: &ExecutionUnit,
    module: &Module,
    workspace: &Workspace,
    shared_hash: &str,
) -> AppResult<String> {
    let mut single_module_unit = unit.clone();
    single_module_unit.modules = vec![module.clone()];
    let argv = render_execution_unit(&single_module_unit, &workspace.root)?;
    let resource_group = render_resource_group(&single_module_unit, &workspace.root)?;
    Ok(component_hash(
        "task",
        [
            unit.profile.clone(),
            unit.task.clone(),
            execution_mode(unit.mode).to_string(),
            command_origin(&unit.command_origin),
            format!("cwd:{}", workspace.root.display()),
            "env:inherit".to_string(),
            format!("toolchain:{}", toolchain_identity(unit, workspace)?),
            format!("cache-record-schema:{CACHE_RECORD_SCHEMA}"),
            format!("resource-group:{resource_group}"),
            format!("argv:{}", argv.join("\u{1f}")),
            format!("shared-inputs:{shared_hash}"),
        ],
    ))
}

fn command_origin(origin: &CommandOrigin) -> String {
    match origin {
        CommandOrigin::DirectArgv => "direct-argv".to_string(),
        CommandOrigin::Preset { name, language } => format!("preset:{language}:{name}"),
    }
}

fn toolchain_identity(unit: &ExecutionUnit, workspace: &Workspace) -> AppResult<String> {
    let mut identity = vec![format!(
        "toven-rustc:{}",
        rskit_version::get_version_info().rust_version
    )];
    match &unit.command_origin {
        CommandOrigin::Preset { language, .. } if language == "rust" => {
            identity.push(command_version(workspace, "cargo", &["--version"])?);
            identity.push(command_version(workspace, "rustc", &["--version"])?);
        }
        CommandOrigin::Preset { language, .. } => {
            identity.push(format!("preset-language:{language}"));
        }
        CommandOrigin::DirectArgv => {
            identity.push("direct-argv".to_string());
        }
    }
    Ok(identity.join("\u{1f}"))
}

fn command_version(workspace: &Workspace, program: &str, args: &[&str]) -> AppResult<String> {
    let output = Command::new(program)
        .current_dir(&workspace.root)
        .args(args)
        .output()
        .map_err(|error| {
            AppError::new(
                ErrorCode::Internal,
                format!("failed to resolve {program} toolchain version"),
            )
            .with_cause(error)
        })?;
    if !output.status.success() {
        return Err(AppError::new(
            ErrorCode::Internal,
            format!(
                "failed to resolve {program} toolchain version with status {}",
                output.status
            ),
        ));
    }
    let stdout = String::from_utf8(output.stdout).map_err(|error| {
        AppError::new(
            ErrorCode::Internal,
            format!("{program} toolchain version was not UTF-8"),
        )
        .with_cause(error)
    })?;
    Ok(format!("{program}:{}", stdout.trim()))
}

const fn execution_mode(mode: ExecutionMode) -> &'static str {
    match mode {
        ExecutionMode::SpawnEach => "spawn-each",
        ExecutionMode::BatchReady => "batch-ready",
        ExecutionMode::WorkspaceOnce => "workspace-once",
    }
}

fn component_hash(name: &str, values: impl IntoIterator<Item = String>) -> String {
    let mut hasher = blake3::Hasher::new();
    hash_field(&mut hasher, name.as_bytes());
    let mut values = values.into_iter().collect::<Vec<_>>();
    values.sort();
    for value in values {
        hash_field(&mut hasher, value.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

fn modules_from_plan(plan: &Plan) -> AppResult<Vec<Module>> {
    let mut modules = BTreeMap::<ModuleId, Module>::new();
    for unit in &plan.units {
        for module in &unit.modules {
            if let Some(existing) = modules.get(&module.name)
                && existing != module
            {
                return Err(AppError::invalid_input(
                    "modules",
                    format!(
                        "conflicting discovered definition for module '{}'",
                        module.name
                    ),
                ));
            }
            modules.insert(module.name.clone(), module.clone());
        }
    }
    Ok(modules.into_values().collect())
}

fn planned_modules(plan: &Plan) -> BTreeMap<PlannedKey, PlannedModule<'_>> {
    let mut planned = BTreeMap::new();
    for unit in &plan.units {
        for module in &unit.modules {
            planned.insert(
                PlannedKey(unit.profile.clone(), module.name.clone()),
                PlannedModule { unit, module },
            );
        }
    }
    planned
}

fn hash_field(hasher: &mut blake3::Hasher, value: &[u8]) {
    hasher.update(&(value.len() as u64).to_be_bytes());
    hasher.update(value);
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rskit_cache::CacheStore;

    use super::{CacheState, TaskCache};
    use crate::cache::key::CacheKey;
    use crate::{
        cache::decision::{CacheMode, prepare_cache_decisions},
        core::{CommandOrigin, ExecutionMode, ExecutionUnit, Module, ModuleId, Plan, Workspace},
    };

    #[test]
    fn corrupt_cache_record_is_deleted_after_read() {
        let root = rskit_testutil::test_workspace!("cache-corrupt-record");
        let cache = TaskCache::new(root.path().join("cache")).expect("cache initializes");
        let key = CacheKey::new("corrupt-record");

        cache
            .runtime
            .block_on(cache.store.set(key.as_str(), "{", None))
            .expect("corrupt value writes");

        assert_eq!(cache.get_record(&key).expect("corrupt read succeeds"), None);
        assert!(
            !cache
                .runtime
                .block_on(cache.store.exists(key.as_str()))
                .expect("cache existence checks")
        );
    }

    #[test]
    fn disabled_cache_decisions_do_not_require_git_workspace() {
        let root = rskit_testutil::test_workspace!("cache-disabled-no-git");
        let plan = plan(root.path().join("not-a-repo"), Vec::new(), false);

        let decisions = prepare_cache_decisions(
            &plan,
            &CacheMode::Disabled {
                reason: "test disabled".to_string(),
            },
            None,
        )
        .expect("disabled cache decisions do not inspect git");

        let decision = decisions
            .get(&(
                "profile".to_string(),
                ModuleId::new("module").expect("module id"),
            ))
            .expect("decision exists");
        assert_eq!(decision.source_hash, "disabled");
        assert_eq!(decision.shared_hash, "disabled");
        assert_eq!(decision.task_hash, "disabled");
    }

    #[test]
    fn passthrough_args_disable_cache_without_git_workspace_when_not_allowed() {
        let root = rskit_testutil::test_workspace!("cache-args-disabled-no-git");
        let plan = plan(
            root.path().join("not-a-repo"),
            vec!["--release".to_string()],
            false,
        );

        let decisions = prepare_cache_decisions(&plan, &CacheMode::ReadWrite, None)
            .expect("passthrough-disabled cache decisions do not inspect git");

        let decision = decisions
            .get(&(
                "profile".to_string(),
                ModuleId::new("module").expect("module id"),
            ))
            .expect("decision exists");
        assert_eq!(
            decision.state,
            CacheState::Disabled {
                reason: "passthrough args disable cache".to_string()
            }
        );
        assert_eq!(decision.source_hash, "disabled");
    }

    #[test]
    fn persistent_units_are_never_cached() {
        let root = rskit_testutil::test_workspace!("cache-persistent-disabled");
        let mut plan = plan(root.path().join("not-a-repo"), Vec::new(), false);
        plan.units[0].persistent = true;

        let decisions = prepare_cache_decisions(&plan, &CacheMode::ReadWrite, None)
            .expect("persistent cache decisions do not inspect git");

        let decision = decisions
            .get(&(
                "profile".to_string(),
                ModuleId::new("module").expect("module id"),
            ))
            .expect("decision exists");
        assert_eq!(
            decision.state,
            CacheState::Disabled {
                reason: "persistent tasks are never cached".to_string()
            }
        );
    }

    fn plan(root: PathBuf, passthrough_args: Vec<String>, cache_args: bool) -> Plan {
        Plan {
            workspace: Workspace {
                schema: 1,
                name: "fixture".to_string(),
                root,
                base_ref: None,
                profiles: Vec::new(),
            },
            units: vec![ExecutionUnit {
                id: "unit".to_string(),
                profile: "profile".to_string(),
                task: "test".to_string(),
                command_origin: CommandOrigin::DirectArgv,
                mode: ExecutionMode::SpawnEach,
                resource_group: String::new(),
                modules: vec![Module {
                    name: ModuleId::new("module").expect("module id"),
                    package: None,
                    root: PathBuf::from("module"),
                    dependencies: Vec::new(),
                    source_patterns: Vec::new(),
                }],
                argv_template: vec!["echo".to_string(), "ok".to_string()],
                module_arg_template: Vec::new(),
                passthrough_args,
                cache_args,
                persistent: false,
                readiness: crate::core::PersistentReadiness::Started,
                readiness_timeout: std::time::Duration::from_secs(30),
                shared_inputs: vec!["Cargo.lock".to_string()],
            }],
        }
    }
}
