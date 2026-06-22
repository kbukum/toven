//! Phase 7 — Schedule: relax edges by `RunStrategy`, level into federated waves,
//! and render one [`PlannedUnit`] per active module.
//!
//! Per-module `RunStrategy` decides whether a module's **intra-ecosystem** ordering
//! edges are kept (`leaf-to-top`) or dropped (`unordered`); **cross-ecosystem
//! overlay edges are never dropped**. The residual active subgraph is topo-levelled
//! into waves, and each active module becomes one per-module unit carrying its
//! rendered argv and the facts the Cache-decision phase needs. (Within-wave
//! BuildTopology/FanOut collapse into processes is an APPLY concern over the
//! immutable per-module `Plan`.)

use std::collections::BTreeMap;

use rskit_errors::{AppError, AppResult};
use toven_model::{
    AbsPath, DepKind, Edge, Graph, Module, ModuleRef, ToolchainTag, Workspace, WorkspaceId,
};
use toven_ports::{CommandTemplate, RunStrategy, Task, TaskKind, TaskVar};

use super::configure::ConfiguredSet;
use super::discover::Federation;
use super::request::PlanRequest;

/// One scheduled, fully rendered unit awaiting its cache verdict.
///
/// Carries the execution facts (`argv`, `persistent`, `workspace`) plus the
/// keying facts (`base_argv`, `shared_inputs`, `cache_args`, `toolchain_identity`)
/// the Cache-decision phase folds into the content key.
#[derive(Debug, Clone)]
pub(super) struct PlannedUnit {
    /// Stable unit id (`ecosystem:name#kind`).
    pub(super) id: String,
    /// Module the unit operates on.
    pub(super) module: ModuleRef,
    /// Task kind name.
    pub(super) kind: String,
    /// Owning workspace (keys the toolchain identity).
    pub(super) workspace: Option<WorkspaceId>,
    /// Fully rendered argv (with passthrough spliced).
    pub(super) argv: Vec<String>,
    /// Whether this unit starts a persistent process.
    pub(super) persistent: bool,
    /// Rendered base argv (without passthrough) — the `task_hash` source.
    pub(super) base_argv: Vec<String>,
    /// Workspace-relative shared-input paths folded into the key.
    pub(super) shared_inputs: Vec<String>,
    /// Whether passthrough args enter the key.
    pub(super) cache_args: bool,
    /// Opaque `tool@version` identity for the owning workspace.
    pub(super) toolchain_identity: String,
}

/// The scheduled units plus the wave-ordered unit ids.
#[derive(Debug, Clone)]
pub(super) struct Scheduled {
    /// All planned units, wave order independent.
    pub(super) units: Vec<PlannedUnit>,
    /// Wave-ordered unit ids (each inner vec is one ready wave).
    pub(super) waves: Vec<Vec<String>>,
}

/// Schedule the active module set into waves of rendered units.
///
/// # Errors
/// An active module with no configured adapter or no task for the intent, a
/// missing workspace a template requires, or a template parse/render failure.
pub(super) fn schedule(
    request: &PlanRequest,
    federation: &Federation,
    active: &[ModuleRef],
    adapters: &ConfiguredSet,
    toolchain: &BTreeMap<WorkspaceId, ToolchainTag>,
) -> AppResult<Scheduled> {
    let active_modules = active_modules(federation, active);
    let strategies = strategies(&active_modules, adapters, &request.intent)?;
    let subgraph = active_subgraph(&active_modules, federation)?;

    let waves = subgraph.waves(|edge| keep_edge(edge, &strategies))?;

    let workspaces = workspace_index(federation);
    let mut units = Vec::new();
    let mut wave_ids = Vec::with_capacity(waves.len());
    for wave in waves {
        let mut ids = Vec::with_capacity(wave.len());
        for reference in wave {
            let module = active_modules.get(&reference).ok_or_else(|| {
                AppError::new(
                    rskit_errors::ErrorCode::Internal,
                    format!("scheduled unknown module '{reference}'"),
                )
            })?;
            let unit = plan_unit(request, module, adapters, toolchain, &workspaces)?;
            ids.push(unit.id.clone());
            units.push(unit);
        }
        wave_ids.push(ids);
    }

    Ok(Scheduled {
        units,
        waves: wave_ids,
    })
}

/// Index the active modules by identity.
fn active_modules(federation: &Federation, active: &[ModuleRef]) -> BTreeMap<ModuleRef, Module> {
    let active: std::collections::BTreeSet<&ModuleRef> = active.iter().collect();
    federation
        .modules
        .iter()
        .filter(|module| active.contains(&module.id))
        .map(|module| (module.id.clone(), module.clone()))
        .collect()
}

/// Resolve each active module's `RunStrategy` (ecosystem override else per-kind default).
fn strategies(
    modules: &BTreeMap<ModuleRef, Module>,
    adapters: &ConfiguredSet,
    intent: &TaskKind,
) -> AppResult<BTreeMap<ModuleRef, RunStrategy>> {
    let mut strategies = BTreeMap::new();
    for reference in modules.keys() {
        let adapter = adapter_for(reference, adapters)?;
        let strategy = adapter
            .common()
            .run_strategy
            .unwrap_or_else(|| adapter.run_strategy_default(intent));
        strategies.insert(reference.clone(), strategy);
    }
    Ok(strategies)
}

/// Build the validated subgraph spanning only the active modules and their edges.
fn active_subgraph(
    modules: &BTreeMap<ModuleRef, Module>,
    federation: &Federation,
) -> AppResult<Graph> {
    let nodes: Vec<Module> = modules.values().cloned().collect();
    let edges: Vec<Edge> = federation
        .edges
        .iter()
        .filter(|edge| modules.contains_key(&edge.from) && modules.contains_key(&edge.to))
        .cloned()
        .collect();
    Graph::build(nodes, edges)
}

/// Whether an edge is kept as an ordering constraint after relaxation.
///
/// Overlay edges are always kept; an intra-ecosystem edge is kept only when its
/// dependent module's strategy is `leaf-to-top`.
fn keep_edge(edge: &Edge, strategies: &BTreeMap<ModuleRef, RunStrategy>) -> bool {
    if edge.kind == DepKind::Overlay {
        return true;
    }
    matches!(strategies.get(&edge.from), Some(RunStrategy::LeafToTop))
}

/// Index discovered workspaces by id.
fn workspace_index(federation: &Federation) -> BTreeMap<WorkspaceId, Workspace> {
    federation
        .workspaces
        .iter()
        .map(|workspace| (workspace.id.clone(), workspace.clone()))
        .collect()
}

/// Render one module into a [`PlannedUnit`].
fn plan_unit(
    request: &PlanRequest,
    module: &Module,
    adapters: &ConfiguredSet,
    toolchain: &BTreeMap<WorkspaceId, ToolchainTag>,
    workspaces: &BTreeMap<WorkspaceId, Workspace>,
) -> AppResult<PlannedUnit> {
    let adapter = adapter_for(&module.id, adapters)?;
    let task = select_task(adapter.default_tasks(), &request.intent).ok_or_else(|| {
        AppError::invalid_input(
            "tasks",
            format!(
                "ecosystem '{}' has no '{}' task",
                module.id.ecosystem,
                request.intent.name()
            ),
        )
    })?;

    let workspace = match &module.workspace {
        Some(id) => Some(workspaces.get(id).ok_or_else(|| {
            AppError::new(
                rskit_errors::ErrorCode::Internal,
                format!("module '{}' references unknown workspace '{id}'", module.id),
            )
        })?),
        None => None,
    };

    let template = CommandTemplate::parse(&task.argv, &task.selector)?;
    let argv = template.render(&request.passthrough, |var| {
        resolve(var, module, workspace, &request.project_root)
    })?;
    let base_argv = template.render(&[], |var| {
        resolve(var, module, workspace, &request.project_root)
    })?;

    let toolchain_identity = match &module.workspace {
        Some(id) => {
            let tag = toolchain.get(id).ok_or_else(|| {
                AppError::new(
                    rskit_errors::ErrorCode::Internal,
                    format!(
                        "module '{}' workspace '{id}' has no resolved toolchain identity",
                        module.id
                    ),
                )
            })?;
            toolchain_identity(tag)
        }
        None => String::new(),
    };

    Ok(PlannedUnit {
        id: format!("{}#{}", module.id, task.kind.name()),
        module: module.id.clone(),
        kind: task.kind.name().to_string(),
        workspace: module.workspace.clone(),
        argv,
        persistent: task.persistent,
        base_argv,
        shared_inputs: task.shared_inputs.clone(),
        cache_args: task.cache_args,
        toolchain_identity,
    })
}

/// Select the adapter default task matching the intent kind (no named extra).
fn select_task(tasks: Vec<Task>, intent: &TaskKind) -> Option<Task> {
    tasks
        .into_iter()
        .find(|task| &task.kind == intent && task.name.is_none())
}

/// The `tool@version` cache identity for a resolved toolchain tag.
fn toolchain_identity(tag: &ToolchainTag) -> String {
    format!("{}@{}", tag.tool, tag.version.as_deref().unwrap_or(""))
}

/// Look up the configured adapter that owns a module's ecosystem.
fn adapter_for<'a>(
    reference: &ModuleRef,
    adapters: &'a ConfiguredSet,
) -> AppResult<&'a dyn toven_ports::ConfiguredAdapter> {
    adapters
        .get(&reference.ecosystem)
        .map(AsRef::as_ref)
        .ok_or_else(|| {
            AppError::new(
                rskit_errors::ErrorCode::Internal,
                format!(
                    "no configured adapter for ecosystem '{}'",
                    reference.ecosystem
                ),
            )
        })
}

/// Resolve one non-splice [`TaskVar`] for a module's argv render.
fn resolve(
    var: TaskVar,
    module: &Module,
    workspace: Option<&Workspace>,
    project_root: &AbsPath,
) -> AppResult<String> {
    let value = match var {
        TaskVar::ProjectRoot => project_root.as_path().display().to_string(),
        TaskVar::WorkspaceRoot => workspace
            .ok_or_else(|| {
                AppError::invalid_input(
                    "task.argv",
                    format!(
                        "module '{}' has no workspace for '{{workspace.root}}'",
                        module.id
                    ),
                )
            })?
            .root
            .as_path()
            .display()
            .to_string(),
        TaskVar::ModuleName => module.id.name.clone(),
        TaskVar::ModulePackage => module
            .package
            .clone()
            .unwrap_or_else(|| module.id.name.clone()),
        TaskVar::ModuleRoot => module.root.as_path().display().to_string(),
        TaskVar::ModuleManifest => module
            .manifest
            .as_ref()
            .ok_or_else(|| {
                AppError::invalid_input(
                    "task.argv",
                    format!(
                        "module '{}' has no manifest for '{{module.manifest}}'",
                        module.id
                    ),
                )
            })?
            .as_path()
            .display()
            .to_string(),
        TaskVar::ModuleSelector | TaskVar::Args => {
            return Err(AppError::new(
                rskit_errors::ErrorCode::Internal,
                format!("splice variable '{var}' must be handled by the template, not resolved"),
            ));
        }
    };
    Ok(value)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use toven_model::{
        AbsPath, DepKind, EcosystemId, Edge, Module, ModuleRef, RepoPath, ToolchainTag, Workspace,
        WorkspaceId,
    };
    use toven_ports::{ConfiguredAdapter, DiscoverResponse, FanOut, RunStrategy, Task, TaskKind};
    use toven_testkit::FakeConfiguredAdapter;

    use super::{ConfiguredSet, schedule};
    use crate::plan::discover::Federation;
    use crate::plan::request::PlanRequest;

    fn eid(id: &str) -> EcosystemId {
        EcosystemId::new(id).unwrap()
    }

    fn mref(ecosystem: &str, name: &str) -> ModuleRef {
        ModuleRef::new(eid(ecosystem), name).unwrap()
    }

    fn module(ecosystem: &str, name: &str, workspace: &str) -> Module {
        let mut module = Module::new(mref(ecosystem, name), RepoPath::new(name).unwrap());
        module.workspace = Some(WorkspaceId::new(workspace).unwrap());
        module
    }

    fn workspace(id: &str) -> Workspace {
        Workspace::new(
            WorkspaceId::new(id).unwrap(),
            RepoPath::new(".").unwrap(),
            ToolchainTag::new("cargo"),
        )
    }

    fn adapter(ecosystem: &str, strategy: RunStrategy) -> Box<dyn ConfiguredAdapter> {
        let task = Task::new(
            TaskKind::Test,
            vec!["x".to_string()],
            FanOut::WholeWorkspace,
        );
        Box::new(
            FakeConfiguredAdapter::new(eid(ecosystem))
                .with_response(DiscoverResponse::new(eid(ecosystem)))
                .with_tasks(vec![task])
                .with_run_strategy(strategy),
        )
    }

    fn request() -> PlanRequest {
        PlanRequest::new("r", "t", TaskKind::Test, AbsPath::new("/repo").unwrap())
    }

    fn toolchains(federation: &Federation) -> BTreeMap<WorkspaceId, ToolchainTag> {
        federation
            .workspaces
            .iter()
            .map(|workspace| {
                (
                    workspace.id.clone(),
                    workspace.toolchain.clone().with_version("v1"),
                )
            })
            .collect()
    }

    fn waves_for(federation: &Federation, adapters: &ConfiguredSet) -> Vec<Vec<String>> {
        let active: Vec<ModuleRef> = federation.modules.iter().map(|m| m.id.clone()).collect();
        schedule(
            &request(),
            federation,
            &active,
            adapters,
            &toolchains(federation),
        )
        .unwrap()
        .waves
    }

    #[test]
    fn workspace_module_without_resolved_toolchain_is_rejected() {
        let federation = Federation {
            workspaces: vec![workspace("rust")],
            modules: vec![module("rust", "app", "rust")],
            edges: Vec::new(),
            warnings: Vec::new(),
        };
        let mut adapters = ConfiguredSet::new();
        adapters.insert(eid("rust"), adapter("rust", RunStrategy::Unordered));

        let active = vec![mref("rust", "app")];
        // Empty toolchain map: the workspace-owning module has no resolved
        // identity, which must fail closed rather than key against an empty one.
        let result = schedule(
            &request(),
            &federation,
            &active,
            &adapters,
            &BTreeMap::new(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn leaf_to_top_orders_dependencies_before_dependents() {
        let federation = Federation {
            workspaces: vec![workspace("rust")],
            modules: vec![
                module("rust", "app", "rust"),
                module("rust", "errors", "rust"),
            ],
            edges: vec![Edge::new(
                mref("rust", "app"),
                mref("rust", "errors"),
                DepKind::Normal,
            )],
            warnings: Vec::new(),
        };
        let mut adapters = ConfiguredSet::new();
        adapters.insert(eid("rust"), adapter("rust", RunStrategy::LeafToTop));

        assert_eq!(
            waves_for(&federation, &adapters),
            vec![
                vec!["rust:errors#test".to_string()],
                vec!["rust:app#test".to_string()],
            ]
        );
    }

    #[test]
    fn unordered_collapses_intra_ecosystem_edges_into_one_wave() {
        let federation = Federation {
            workspaces: vec![workspace("rust")],
            modules: vec![
                module("rust", "app", "rust"),
                module("rust", "errors", "rust"),
            ],
            edges: vec![Edge::new(
                mref("rust", "app"),
                mref("rust", "errors"),
                DepKind::Normal,
            )],
            warnings: Vec::new(),
        };
        let mut adapters = ConfiguredSet::new();
        adapters.insert(eid("rust"), adapter("rust", RunStrategy::Unordered));

        assert_eq!(
            waves_for(&federation, &adapters),
            vec![vec![
                "rust:app#test".to_string(),
                "rust:errors#test".to_string()
            ]]
        );
    }

    #[test]
    fn overlay_edges_are_never_dropped_even_under_unordered() {
        let federation = Federation {
            workspaces: vec![workspace("go"), workspace("rust")],
            modules: vec![module("go", "api", "go"), module("rust", "shared", "rust")],
            edges: vec![Edge::new(
                mref("go", "api"),
                mref("rust", "shared"),
                DepKind::Overlay,
            )],
            warnings: Vec::new(),
        };
        let mut adapters = ConfiguredSet::new();
        adapters.insert(eid("go"), adapter("go", RunStrategy::Unordered));
        adapters.insert(eid("rust"), adapter("rust", RunStrategy::Unordered));

        // The overlay still orders shared before api despite both being unordered.
        assert_eq!(
            waves_for(&federation, &adapters),
            vec![
                vec!["rust:shared#test".to_string()],
                vec!["go:api#test".to_string()],
            ]
        );
    }
}
