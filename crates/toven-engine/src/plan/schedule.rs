//! Schedule: relax edges by `RunStrategy`, level into federated waves, group the
//! active modules by intrinsic `FanOut`, and render one [`PlannedUnit`] per group.
//!
//! Per-module `RunStrategy` decides whether a module's **intra-ecosystem** ordering
//! edges are kept (`leaf-to-top`) or dropped (`unordered`); **cross-ecosystem
//! overlay edges are never dropped**. The residual active subgraph is topo-levelled
//! into waves. Modules are then grouped by the task's [`FanOut`]: a `PerModule` task
//! yields one unit per module, while `Batchable`/`WholeWorkspace` tasks collapse all
//! same-ecosystem-and-workspace modules into a single invocation (selectors are
//! repeated for `Batchable`, omitted for `WholeWorkspace`). Grouping by workspace
//! keeps a collapsed unit's `{workspace.root}` and toolchain identity valid for every
//! member. A collapsed group may span several waves; it is scheduled in the latest
//! wave any of its members occupy, so every gated dependency has already run. Each
//! unit carries the rendered argv and the facts the Cache-decision phase needs.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use rskit_errors::{AppError, AppResult};
use toven_model::{
    AbsPath, DepKind, Edge, ExecutionReadiness, Graph, Module, ModuleKey, ToolchainTag, Workspace,
    WorkspaceId,
};
use toven_ports::{
    CommandTemplate, FanOut, Readiness, RunStrategy, Task, TaskKind, TaskOrigin, TaskVar,
    merge_task,
};

use super::configure::MemberAdapters;
use super::discover::Federation;
use super::overrides::GroupOverrides;
use super::request::PlanRequest;

/// One scheduled, fully rendered unit awaiting its cache verdict.
///
/// Carries the execution facts (`argv`, `persistent`, `workspace`) plus the
/// keying facts (`base_argv`, `shared_inputs`, `cache_args`, `toolchain_identity`)
/// the Cache-decision phase folds into the content key.
#[derive(Debug, Clone)]
pub(super) struct PlannedUnit {
    /// Stable unit id (`ecosystem:name#kind`, member-prefixed under a federation;
    /// batched/whole-workspace units drop the module name and key by workspace:
    /// `ecosystem@workspace#kind`, or `ecosystem#kind` when workspace-less).
    pub(super) id: String,
    /// Representative module the unit operates on.
    pub(super) module: ModuleKey,
    /// Every module collapsed into this unit (always non-empty, contains `module`).
    pub(super) members: Vec<ModuleKey>,
    /// Task kind name.
    pub(super) kind: String,
    /// Provenance of the resolved task (which config layer won).
    pub(super) origin: TaskOrigin,
    /// Owning workspace (keys the toolchain identity).
    pub(super) workspace: Option<WorkspaceId>,
    /// Fully rendered argv (with passthrough spliced).
    pub(super) argv: Vec<String>,
    /// Whether this unit starts a persistent process.
    pub(super) persistent: bool,
    /// Persistent readiness signal.
    pub(super) readiness: ExecutionReadiness,
    /// Persistent readiness timeout.
    pub(super) readiness_timeout: Duration,
    /// Rendered base argv (without passthrough) — the `task_hash` source.
    pub(super) base_argv: Vec<String>,
    /// Workspace-relative shared-input paths folded into the key.
    pub(super) shared_inputs: Vec<String>,
    /// Whether passthrough args enter the key.
    pub(super) cache_args: bool,
    /// Opaque `tool@version` identity for the owning workspace.
    pub(super) toolchain_identity: String,
    /// Unit ids this unit depends on (scheduled dependency edges) for gating.
    pub(super) depends_on: Vec<String>,
    /// Optional within-wave serialization key from the module metadata.
    pub(super) resource_group: Option<String>,
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
/// `overrides` carries any `[groups.*]` scope overrides: a group's `run_strategy`
/// wins over the ecosystem default for its members, and a group's `tasks` entry
/// field-merges onto the ecosystem/adapter default for the intent (marking the
/// resolved task [`TaskOrigin::Group`]). An overridden batch unit is kept distinct
/// from the un-overridden default so members never collapse across differing argv.
///
/// # Errors
/// An active module with no configured adapter or no task for the intent, a
/// missing workspace a template requires, or a template parse/render failure.
pub(super) fn schedule(
    request: &PlanRequest,
    federation: &Federation,
    active: &[ModuleKey],
    adapters: &MemberAdapters,
    overrides: &GroupOverrides,
    toolchain: &BTreeMap<WorkspaceId, ToolchainTag>,
) -> AppResult<Scheduled> {
    let active_modules = active_modules(federation, active);
    let effective = effective_tasks(&active_modules, adapters, &request.intent, overrides)?;
    let strategies = strategies(&active_modules, adapters, overrides, &request.intent)?;
    let subgraph = active_subgraph(&active_modules, federation)?;

    let waves = subgraph.waves(|edge| keep_edge(edge, &strategies))?;
    let kept_deps = kept_dependencies(&active_modules, federation, &strategies);

    let workspaces = workspace_index(federation);
    let group_ids = group_id_map(&active_modules, &effective)?;
    let mut units = Vec::new();
    let mut group_members: BTreeMap<String, Vec<ModuleKey>> = BTreeMap::new();
    let mut group_wave: BTreeMap<String, usize> = BTreeMap::new();
    let mut wave_order: Vec<String> = Vec::new();
    for (index, wave) in waves.into_iter().enumerate() {
        for reference in wave {
            let id = group_ids.get(&reference).ok_or_else(|| {
                AppError::new(
                    rskit_errors::ErrorCode::Internal,
                    format!("scheduled unknown module '{reference}'"),
                )
            })?;
            if !group_members.contains_key(id) {
                wave_order.push(id.clone());
            }
            group_members.entry(id.clone()).or_default().push(reference);
            group_wave
                .entry(id.clone())
                .and_modify(|w| *w = (*w).max(index))
                .or_insert(index);
        }
    }
    let last_wave = group_wave.values().copied().max().map_or(0, |w| w + 1);
    let mut wave_ids: Vec<Vec<String>> = vec![Vec::new(); last_wave];
    for id in wave_order {
        let members = &group_members[&id];
        let unit = plan_unit(
            request,
            &id,
            members,
            &active_modules,
            &effective,
            toolchain,
            &workspaces,
            &kept_deps,
            &group_ids,
        )?;
        wave_ids[group_wave[&id]].push(unit.id.clone());
        units.push(unit);
    }
    wave_ids.retain(|wave| !wave.is_empty());

    Ok(Scheduled {
        units,
        waves: wave_ids,
    })
}

/// The unit id for `module` under `kind` (`ecosystem:name#kind`, member-prefixed
/// whenever the module belongs to a federation member via [`ModuleKey`]'s
/// `Display`).
fn unit_id(module: &ModuleKey, kind: &str) -> String {
    format!("{module}#{kind}")
}

/// The id of the unit a module belongs to: its own per-module id for a
/// `PerModule` task, or a shared group id for `Batchable`/`WholeWorkspace` tasks.
///
/// A batch group id is keyed by `member`, `ecosystem`, **and owning workspace**
/// (`[member/]ecosystem@workspace#kind`, or `[member/]ecosystem#kind` for a
/// workspace-less module). Keeping the workspace in the key guarantees a collapsed
/// unit never spans workspaces, so the representative's `{workspace.root}` and
/// resolved toolchain identity are valid for every member it carries. When a group
/// task override applies, its scope-qualified identity is folded into the key
/// (`…~identity…`) so members carrying overrides from different declarations — or
/// none — never collapse into one argv.
fn group_id(
    key: &ModuleKey,
    module: &Module,
    kind: &str,
    fan_out: FanOut,
    override_group: Option<&str>,
) -> String {
    if fan_out == FanOut::PerModule {
        return unit_id(key, kind);
    }
    let ecosystem = &key.module().ecosystem;
    let base = module
        .workspace
        .as_ref()
        .map_or_else(|| ecosystem.to_string(), |ws| format!("{ecosystem}@{ws}"));
    let scope = override_group.map_or_else(|| base.clone(), |group| format!("{base}~{group}"));
    key.member().map_or_else(
        || format!("{scope}#{kind}"),
        |member| format!("{member}/{scope}#{kind}"),
    )
}

/// A module's resolved task for the intent, with the group (if any) whose
/// override produced it.
struct EffectiveTask {
    /// The adapter default field-merged with any group override.
    task: Task,
    /// The declaring group when a `[groups.*].tasks` override applied.
    group: Option<String>,
}

/// Resolve every active module's effective task for the intent: the adapter
/// default, field-merged with the module's group task override when one applies.
fn effective_tasks(
    modules: &BTreeMap<ModuleKey, Module>,
    adapters: &MemberAdapters,
    intent: &TaskKind,
    overrides: &GroupOverrides,
) -> AppResult<BTreeMap<ModuleKey, EffectiveTask>> {
    let mut resolved = BTreeMap::new();
    for (key, module) in modules {
        let adapter = adapter_for(module, adapters)?;
        let default_tasks = adapter.default_tasks();
        let default = select_task(&default_tasks, intent).ok_or_else(|| {
            unknown_task_error(&module.id.ecosystem.to_string(), intent, &default_tasks)
        })?;
        let effective = match overrides.task(key, intent.name()) {
            Some((group, over)) => {
                let mut merged = merge_task(&default, over);
                merged.origin = TaskOrigin::Group;
                EffectiveTask {
                    task: merged,
                    group: Some(group.to_string()),
                }
            }
            None => EffectiveTask {
                task: default,
                group: None,
            },
        };
        resolved.insert(key.clone(), effective);
    }
    Ok(resolved)
}

/// Build the typed error for an intent that no task in `ecosystem` satisfies,
/// enriched with the nearest resolvable task name and a discovery hint.
///
/// The candidate set is the ecosystem's resolved task names (canonical, so
/// `format` not `fmt`); a nearest match within the default edit-distance is
/// offered as advisory data in the message. The error stays a typed
/// [`AppError`] — the CLI's renderer is what prints it.
fn unknown_task_error(ecosystem: &str, intent: &TaskKind, available: &[Task]) -> AppError {
    let names: Vec<String> = available.iter().map(task_addressable_name).collect();
    let wanted = intent.name();
    let suggestion = rskit_util::strings::nearest(wanted, names.iter().map(String::as_str))
        .map_or_else(String::new, |name| format!(" Did you mean '{name}'?"));
    AppError::invalid_input(
        "tasks",
        format!(
            "ecosystem '{ecosystem}' has no '{wanted}' task.{suggestion} Run 'toven tasks' to list every runnable task."
        ),
    )
}

/// The user-addressable canonical name of a resolved task (the explicit name for
/// a named extra, else the built-in kind's canonical name).
fn task_addressable_name(task: &Task) -> String {
    task.name
        .clone()
        .unwrap_or_else(|| task.kind.name().to_string())
}

/// Map every active module to the id of the unit that will carry it.
fn group_id_map(
    modules: &BTreeMap<ModuleKey, Module>,
    effective: &BTreeMap<ModuleKey, EffectiveTask>,
) -> AppResult<BTreeMap<ModuleKey, String>> {
    let mut ids = BTreeMap::new();
    for (key, module) in modules {
        let eff = effective_for(key, effective)?;
        ids.insert(
            key.clone(),
            group_id(
                key,
                module,
                eff.task.kind.name(),
                eff.task.fan_out,
                eff.group.as_deref(),
            ),
        );
    }
    Ok(ids)
}

/// Look up a module's resolved effective task, failing closed on an unknown key.
fn effective_for<'a>(
    key: &ModuleKey,
    effective: &'a BTreeMap<ModuleKey, EffectiveTask>,
) -> AppResult<&'a EffectiveTask> {
    effective.get(key).ok_or_else(|| {
        AppError::new(
            rskit_errors::ErrorCode::Internal,
            format!("scheduled unknown module '{key}'"),
        )
    })
}

/// Map each active module to the unit ids of its kept dependency edges.
///
/// The kept edges are exactly those that ordered the waves (overlay edges plus
/// intra-ecosystem edges retained under `leaf-to-top`); they drive APPLY's
/// fail-closed gating. All endpoints are active, so every id resolves to a unit.
fn kept_dependencies(
    modules: &BTreeMap<ModuleKey, Module>,
    federation: &Federation,
    strategies: &BTreeMap<ModuleKey, RunStrategy>,
) -> BTreeMap<ModuleKey, Vec<ModuleKey>> {
    let mut deps: BTreeMap<ModuleKey, Vec<ModuleKey>> = BTreeMap::new();
    for edge in &federation.edges {
        if modules.contains_key(&edge.from)
            && modules.contains_key(&edge.to)
            && keep_edge(edge, strategies)
        {
            deps.entry(edge.from.clone())
                .or_default()
                .push(edge.to.clone());
        }
    }
    deps
}

/// Index the active modules by key.
fn active_modules(federation: &Federation, active: &[ModuleKey]) -> BTreeMap<ModuleKey, Module> {
    let active: std::collections::BTreeSet<&ModuleKey> = active.iter().collect();
    federation
        .modules
        .iter()
        .filter(|module| active.contains(&module.key()))
        .map(|module| (module.key(), module.clone()))
        .collect()
}

/// Resolve each active module's `RunStrategy`: a group override wins, else the
/// ecosystem override, else the per-kind adapter default.
fn strategies(
    modules: &BTreeMap<ModuleKey, Module>,
    adapters: &MemberAdapters,
    overrides: &GroupOverrides,
    intent: &TaskKind,
) -> AppResult<BTreeMap<ModuleKey, RunStrategy>> {
    let mut strategies = BTreeMap::new();
    for (key, module) in modules {
        let adapter = adapter_for(module, adapters)?;
        let strategy = overrides
            .run_strategy(key)
            .or_else(|| adapter.common().run_strategy)
            .unwrap_or_else(|| adapter.run_strategy_default(intent));
        strategies.insert(key.clone(), strategy);
    }
    Ok(strategies)
}

/// Build the validated subgraph spanning only the active modules and their edges.
fn active_subgraph(
    modules: &BTreeMap<ModuleKey, Module>,
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
fn keep_edge(edge: &Edge, strategies: &BTreeMap<ModuleKey, RunStrategy>) -> bool {
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

/// Render a group of modules collapsed into one [`PlannedUnit`].
///
/// `members` is the set of modules sharing the group `id` in first-seen wave order
/// (a single module for `PerModule`, all same-ecosystem-and-workspace modules for
/// `Batchable`/`WholeWorkspace`). A batched group may span several waves; it is
/// scheduled in the latest wave its members occupy. Argv is rendered once from the
/// representative member: `Batchable` repeats each member's selector fragment,
/// `WholeWorkspace` omits the selector.
#[allow(clippy::too_many_arguments)]
fn plan_unit(
    request: &PlanRequest,
    id: &str,
    members: &[ModuleKey],
    active_modules: &BTreeMap<ModuleKey, Module>,
    effective: &BTreeMap<ModuleKey, EffectiveTask>,
    toolchain: &BTreeMap<WorkspaceId, ToolchainTag>,
    workspaces: &BTreeMap<WorkspaceId, Workspace>,
    kept_deps: &BTreeMap<ModuleKey, Vec<ModuleKey>>,
    group_ids: &BTreeMap<ModuleKey, String>,
) -> AppResult<PlannedUnit> {
    let representative = active_modules.get(&members[0]).ok_or_else(|| {
        AppError::new(
            rskit_errors::ErrorCode::Internal,
            format!("scheduled unknown module '{}'", members[0]),
        )
    })?;
    let task = &effective_for(&members[0], effective)?.task;

    let template = CommandTemplate::parse(&task.argv, &task.selector)?;
    let modules = member_modules(members, active_modules)?;
    let workspaces_for = member_workspaces(&modules, workspaces)?;
    let argv = render_argv(
        &template,
        &request.passthrough,
        &modules,
        &workspaces_for,
        request,
    )?;
    let base_argv = render_argv(&template, &[], &modules, &workspaces_for, request)?;

    let toolchain_identity = resolve_toolchain_identity(representative, toolchain)?;
    let kind_name = task.kind.name().to_string();
    super::shared_inputs::validate_shared_inputs(id, &task.shared_inputs)?;
    let depends_on = group_dependencies(id, members, kept_deps, group_ids);
    let resource_group = representative.resource_group.clone();

    Ok(PlannedUnit {
        id: id.to_string(),
        module: members[0].clone(),
        members: members.to_vec(),
        kind: kind_name,
        origin: task.origin,
        workspace: representative.workspace.clone(),
        argv,
        persistent: task.persistent,
        readiness: readiness(&task.readiness),
        readiness_timeout: task.readiness_timeout,
        base_argv,
        shared_inputs: task.shared_inputs.clone(),
        cache_args: task.cache_args,
        toolchain_identity,
        depends_on,
        resource_group,
    })
}

/// Resolve each member key to its module, failing closed on an unknown key.
fn member_modules<'a>(
    members: &[ModuleKey],
    active_modules: &'a BTreeMap<ModuleKey, Module>,
) -> AppResult<Vec<&'a Module>> {
    members
        .iter()
        .map(|key| {
            active_modules.get(key).ok_or_else(|| {
                AppError::new(
                    rskit_errors::ErrorCode::Internal,
                    format!("scheduled unknown module '{key}'"),
                )
            })
        })
        .collect()
}

/// Resolve each module's owning workspace (none when the module is workspace-less).
fn member_workspaces<'a>(
    modules: &[&Module],
    workspaces: &'a BTreeMap<WorkspaceId, Workspace>,
) -> AppResult<Vec<Option<&'a Workspace>>> {
    modules
        .iter()
        .map(|module| {
            module.workspace.as_ref().map_or_else(
                || Ok(None),
                |id| {
                    workspaces.get(id).map(Some).ok_or_else(|| {
                        AppError::new(
                            rskit_errors::ErrorCode::Internal,
                            format!("module '{}' references unknown workspace '{id}'", module.id),
                        )
                    })
                },
            )
        })
        .collect()
}

/// The `tool@version` cache identity for the representative module's workspace.
fn resolve_toolchain_identity(
    representative: &Module,
    toolchain: &BTreeMap<WorkspaceId, ToolchainTag>,
) -> AppResult<String> {
    let Some(id) = &representative.workspace else {
        return Ok(String::new());
    };
    let tag = toolchain.get(id).ok_or_else(|| {
        AppError::new(
            rskit_errors::ErrorCode::Internal,
            format!(
                "module '{}' workspace '{id}' has no resolved toolchain identity",
                representative.id
            ),
        )
    })?;
    Ok(toolchain_identity(tag))
}

/// The de-duplicated dependency-group ids a unit gates on (excluding itself).
///
/// Order is the first-seen order across `members`; a `BTreeSet` guards membership
/// so de-duplication stays linear rather than quadratic in the edge count.
fn group_dependencies(
    id: &str,
    members: &[ModuleKey],
    kept_deps: &BTreeMap<ModuleKey, Vec<ModuleKey>>,
    group_ids: &BTreeMap<ModuleKey, String>,
) -> Vec<String> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut depends_on: Vec<String> = Vec::new();
    for member in members {
        if let Some(deps) = kept_deps.get(member) {
            for dep in deps {
                if let Some(dep_id) = group_ids.get(dep)
                    && dep_id != id
                    && seen.insert(dep_id.as_str())
                {
                    depends_on.push(dep_id.clone());
                }
            }
        }
    }
    depends_on
}

/// Render a group's argv, batching every member's selector fragment.
fn render_argv(
    template: &CommandTemplate,
    passthrough: &[String],
    modules: &[&Module],
    workspaces: &[Option<&Workspace>],
    request: &PlanRequest,
) -> AppResult<Vec<String>> {
    let mut resolvers: Vec<_> = modules
        .iter()
        .zip(workspaces)
        .map(|(module, workspace)| {
            move |var: TaskVar| resolve(var, module, *workspace, &request.project_root)
        })
        .collect();
    template.render_batch(passthrough, &mut resolvers)
}

/// Convert the adapter task readiness vocabulary into immutable plan vocabulary.
fn readiness(readiness: &Readiness) -> ExecutionReadiness {
    match readiness {
        Readiness::Started => ExecutionReadiness::Started,
        Readiness::Command(argv) => ExecutionReadiness::Command(argv.clone()),
        Readiness::OutputContains(value) => ExecutionReadiness::OutputContains(value.clone()),
    }
}

/// Select the adapter default task matching the intent kind (no named extra).
fn select_task(tasks: &[Task], intent: &TaskKind) -> Option<Task> {
    tasks
        .iter()
        .find(|task| &task.kind == intent && task.name.is_none())
        .cloned()
}

/// The `tool@version` cache identity for a resolved toolchain tag.
fn toolchain_identity(tag: &ToolchainTag) -> String {
    format!("{}@{}", tag.tool, tag.version.as_deref().unwrap_or(""))
}

/// Look up the configured adapter that owns a module within its member.
fn adapter_for<'a>(
    module: &Module,
    adapters: &'a MemberAdapters,
) -> AppResult<&'a dyn toven_ports::ConfiguredAdapter> {
    adapters
        .get(module.member.as_ref(), &module.id.ecosystem)
        .ok_or_else(|| {
            AppError::new(
                rskit_errors::ErrorCode::Internal,
                format!(
                    "no configured adapter for ecosystem '{}'",
                    module.id.ecosystem
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

    use super::super::configure::{ConfiguredSet, MemberAdapters};
    use super::super::overrides::GroupOverrides;
    use super::{schedule, unknown_task_error};
    use crate::plan::discover::Federation;
    use crate::plan::request::PlanRequest;

    #[test]
    fn unknown_task_error_suggests_the_nearest_name_and_discovery_hint() {
        let available = vec![
            Task::new(
                TaskKind::Format,
                vec!["cargo".into(), "fmt".into()],
                FanOut::WholeWorkspace,
            ),
            Task::new(
                TaskKind::Test,
                vec!["cargo".into(), "test".into()],
                FanOut::Batchable,
            ),
        ];
        let error = unknown_task_error("rust", &TaskKind::Custom("fmt".into()), &available);
        let message = error.to_string();
        assert!(message.contains("has no 'fmt' task"), "{message}");
        assert!(message.contains("Did you mean 'format'?"), "{message}");
        assert!(message.contains("toven tasks"), "{message}");
    }

    #[test]
    fn unknown_task_error_omits_a_suggestion_for_a_far_off_name() {
        let available = vec![Task::new(
            TaskKind::Test,
            vec!["cargo".into(), "test".into()],
            FanOut::Batchable,
        )];
        let error = unknown_task_error("rust", &TaskKind::Custom("zzzzzz".into()), &available);
        let message = error.to_string();
        assert!(!message.contains("Did you mean"), "{message}");
        assert!(message.contains("toven tasks"), "{message}");
    }

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
        adapter_with(ecosystem, strategy, FanOut::PerModule)
    }

    fn adapter_with(
        ecosystem: &str,
        strategy: RunStrategy,
        fan_out: FanOut,
    ) -> Box<dyn ConfiguredAdapter> {
        let task = Task::new(TaskKind::Test, vec!["x".to_string()], fan_out);
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

    fn single_member(set: ConfiguredSet) -> MemberAdapters {
        let mut adapters = MemberAdapters::default();
        adapters.insert(None, set);
        adapters
    }

    fn waves_for(federation: &Federation, adapters: &MemberAdapters) -> Vec<Vec<String>> {
        let active: Vec<toven_model::ModuleKey> =
            federation.modules.iter().map(Module::key).collect();
        schedule(
            &request(),
            federation,
            &active,
            adapters,
            &GroupOverrides::default(),
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
        let adapters = single_member(adapters);

        let active = vec![toven_model::ModuleKey::bare(mref("rust", "app"))];
        // Empty toolchain map: the workspace-owning module has no resolved
        // identity, which must fail closed rather than key against an empty one.
        let result = schedule(
            &request(),
            &federation,
            &active,
            &adapters,
            &GroupOverrides::default(),
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
            waves_for(&federation, &single_member(adapters)),
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
            waves_for(&federation, &single_member(adapters)),
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
            waves_for(&federation, &single_member(adapters)),
            vec![
                vec!["rust:shared#test".to_string()],
                vec!["go:api#test".to_string()],
            ]
        );
    }

    #[test]
    fn whole_workspace_collapses_modules_into_one_unit() {
        let federation = Federation {
            workspaces: vec![workspace("rust")],
            modules: vec![
                module("rust", "app", "rust"),
                module("rust", "errors", "rust"),
            ],
            edges: Vec::new(),
            warnings: Vec::new(),
        };
        let mut adapters = ConfiguredSet::new();
        adapters.insert(
            eid("rust"),
            adapter_with("rust", RunStrategy::Unordered, FanOut::WholeWorkspace),
        );
        assert_eq!(
            waves_for(&federation, &single_member(adapters)),
            vec![vec!["rust@rust#test".to_string()]]
        );
    }

    #[test]
    fn batchable_splits_distinct_workspaces_in_one_ecosystem() {
        // Two Cargo workspaces under the same ecosystem must not collapse into one
        // batched unit: each unit's {workspace.root}/toolchain comes from its
        // representative, so a cross-workspace collapse would mis-render the others.
        let federation = Federation {
            workspaces: vec![workspace("core"), workspace("contrib")],
            modules: vec![
                module("rust", "errors", "core"),
                module("rust", "plugin", "contrib"),
            ],
            edges: Vec::new(),
            warnings: Vec::new(),
        };
        let mut adapters = ConfiguredSet::new();
        adapters.insert(
            eid("rust"),
            adapter_with("rust", RunStrategy::Unordered, FanOut::Batchable),
        );
        let active: Vec<toven_model::ModuleKey> =
            federation.modules.iter().map(Module::key).collect();
        let scheduled = schedule(
            &request(),
            &federation,
            &active,
            &single_member(adapters),
            &GroupOverrides::default(),
            &toolchains(&federation),
        )
        .unwrap();
        assert_eq!(scheduled.units.len(), 2);
        let mut ids: Vec<&str> = scheduled
            .units
            .iter()
            .map(|unit| unit.id.as_str())
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["rust@contrib#test", "rust@core#test"]);
        assert!(scheduled.units.iter().all(|unit| unit.members.len() == 1));
    }

    #[test]
    fn batchable_groups_members_and_keeps_distinct_ecosystems_apart() {
        let federation = Federation {
            workspaces: vec![workspace("go"), workspace("rust")],
            modules: vec![
                module("rust", "app", "rust"),
                module("rust", "errors", "rust"),
                module("go", "api", "go"),
            ],
            edges: Vec::new(),
            warnings: Vec::new(),
        };
        let mut adapters = ConfiguredSet::new();
        adapters.insert(
            eid("rust"),
            adapter_with("rust", RunStrategy::Unordered, FanOut::Batchable),
        );
        adapters.insert(
            eid("go"),
            adapter_with("go", RunStrategy::Unordered, FanOut::Batchable),
        );
        let active: Vec<toven_model::ModuleKey> =
            federation.modules.iter().map(Module::key).collect();
        let scheduled = schedule(
            &request(),
            &federation,
            &active,
            &single_member(adapters),
            &GroupOverrides::default(),
            &toolchains(&federation),
        )
        .unwrap();
        assert_eq!(scheduled.units.len(), 2);
        let rust = scheduled
            .units
            .iter()
            .find(|unit| unit.id == "rust@rust#test")
            .unwrap();
        assert_eq!(rust.members.len(), 2);
    }

    fn task_override(argv: &[&str]) -> toven_ports::TaskOverride {
        toven_ports::TaskOverride {
            argv: Some(argv.iter().map(ToString::to_string).collect()),
            ..toven_ports::TaskOverride::default()
        }
    }

    fn group_overrides(
        name: &str,
        group: &crate::config::GroupConfig,
        members: &[toven_model::ModuleKey],
    ) -> GroupOverrides {
        let mut overrides = GroupOverrides::default();
        overrides
            .record(name, group, &members.iter().cloned().collect())
            .expect("group overrides record");
        overrides
    }

    #[test]
    fn group_task_override_applies_to_members_only() {
        let federation = Federation {
            workspaces: vec![workspace("rust")],
            modules: vec![
                module("rust", "app", "rust"),
                module("rust", "errors", "rust"),
            ],
            edges: Vec::new(),
            warnings: Vec::new(),
        };
        let mut adapters = ConfiguredSet::new();
        adapters.insert(
            eid("rust"),
            adapter_with("rust", RunStrategy::Unordered, FanOut::Batchable),
        );
        let group = crate::config::GroupConfig {
            tasks: BTreeMap::from([("test".to_string(), task_override(&["nextest", "run"]))]),
            ..crate::config::GroupConfig::default()
        };
        let overrides = group_overrides(
            "integration",
            &group,
            &[toven_model::ModuleKey::bare(mref("rust", "app"))],
        );

        let active: Vec<toven_model::ModuleKey> =
            federation.modules.iter().map(Module::key).collect();
        let scheduled = schedule(
            &request(),
            &federation,
            &active,
            &single_member(adapters),
            &overrides,
            &toolchains(&federation),
        )
        .unwrap();

        // The overridden member splits into its own group-tagged unit; the
        // non-member keeps the ecosystem default in the plain batch unit.
        let overridden = scheduled
            .units
            .iter()
            .find(|unit| unit.id == "rust@rust~integration#test")
            .expect("group-tagged unit present");
        assert_eq!(overridden.argv, ["nextest", "run"]);
        assert_eq!(overridden.members, [mref("rust", "app").into()]);
        let default = scheduled
            .units
            .iter()
            .find(|unit| unit.id == "rust@rust#test")
            .expect("default unit present");
        assert_eq!(default.argv, ["x"]);
        assert_eq!(default.members, [mref("rust", "errors").into()]);
    }

    #[test]
    fn same_name_group_overrides_from_distinct_scopes_do_not_collapse() {
        // Two modules in the same batch base, overridden by a member-local group
        // and an umbrella group that share the plain name `integration` but carry
        // different argv. Folding the plain name would collapse them into one
        // `…~integration#test` unit and render argv from the representative only;
        // the scope-qualified identity must keep them in distinct units.
        let federation = Federation {
            workspaces: vec![workspace("rust")],
            modules: vec![
                module("rust", "app", "rust"),
                module("rust", "errors", "rust"),
            ],
            edges: Vec::new(),
            warnings: Vec::new(),
        };
        let mut adapters = ConfiguredSet::new();
        adapters.insert(
            eid("rust"),
            adapter_with("rust", RunStrategy::Unordered, FanOut::Batchable),
        );

        let mut overrides = GroupOverrides::default();
        let local = crate::config::GroupConfig {
            tasks: BTreeMap::from([("test".to_string(), task_override(&["local", "run"]))]),
            ..crate::config::GroupConfig::default()
        };
        overrides
            .record(
                "member.billing.integration",
                &local,
                &std::iter::once(toven_model::ModuleKey::bare(mref("rust", "app"))).collect(),
            )
            .expect("member-local records");
        let umbrella = crate::config::GroupConfig {
            tasks: BTreeMap::from([("test".to_string(), task_override(&["umbrella", "run"]))]),
            ..crate::config::GroupConfig::default()
        };
        overrides
            .record(
                "umbrella.integration",
                &umbrella,
                &std::iter::once(toven_model::ModuleKey::bare(mref("rust", "errors"))).collect(),
            )
            .expect("umbrella records");

        let active: Vec<toven_model::ModuleKey> =
            federation.modules.iter().map(Module::key).collect();
        let scheduled = schedule(
            &request(),
            &federation,
            &active,
            &single_member(adapters),
            &overrides,
            &toolchains(&federation),
        )
        .unwrap();

        let member_local = scheduled
            .units
            .iter()
            .find(|unit| unit.id == "rust@rust~member.billing.integration#test")
            .expect("member-local unit present");
        assert_eq!(member_local.argv, ["local", "run"]);
        assert_eq!(member_local.members, [mref("rust", "app").into()]);
        let umbrella_unit = scheduled
            .units
            .iter()
            .find(|unit| unit.id == "rust@rust~umbrella.integration#test")
            .expect("umbrella unit present");
        assert_eq!(umbrella_unit.argv, ["umbrella", "run"]);
        assert_eq!(umbrella_unit.members, [mref("rust", "errors").into()]);
    }

    #[test]
    fn group_run_strategy_override_relaxes_members_only() {
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
        // Adapter default is dependency-respecting, so without an override the
        // edge orders `errors` before `app` across two waves.
        adapters.insert(eid("rust"), adapter("rust", RunStrategy::LeafToTop));
        let group = crate::config::GroupConfig {
            run_strategy: Some(RunStrategy::Unordered),
            ..crate::config::GroupConfig::default()
        };
        let overrides = group_overrides(
            "flat",
            &group,
            &[toven_model::ModuleKey::bare(mref("rust", "app"))],
        );

        let active: Vec<toven_model::ModuleKey> =
            federation.modules.iter().map(Module::key).collect();
        let waves = schedule(
            &request(),
            &federation,
            &active,
            &single_member(adapters),
            &overrides,
            &toolchains(&federation),
        )
        .unwrap()
        .waves;

        // The dependent's `unordered` override drops its intra-ecosystem edge, so
        // both modules collapse into a single wave.
        assert_eq!(waves.len(), 1);
    }
}
