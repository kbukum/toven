//! Execution unit planning.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    adapter::AdapterRegistry,
    core::{
        AdapterId, AppError, AppResult, CommandOrigin, DISCOVERY_SCHEMA_VERSION, DiscoverRequest,
        ExecutionMode, ExecutionUnit, Plan, Profile, ScopeId, ScopeOverride, ScopedModuleKey, Task,
        TaskCommand, Workspace, scoped_module_key, validate_discovery_response,
    },
    engine::{
        graph::{ResolvedDependencyGraph, resolve_dependency_graph},
        scheduler::split_wave_by_manifest,
    },
    exec::{render_execution_unit, render_resource_group},
};

/// Modules discovered for one profile/task pair.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DiscoveredTaskProfile {
    /// Profile that owns the task.
    pub profile: Profile,
    /// Scope override that owns this planned partition.
    pub scope_id: ScopeId,
    /// Adapter that owns this planned partition.
    pub adapter_id: AdapterId,
    /// Task selected from the profile.
    pub task: Task,
    /// Modules discovered for the profile.
    pub modules: Vec<crate::core::Module>,
}

/// Build a task plan for every profile that defines `task_name`.
pub fn plan_workspace(
    workspace: Workspace,
    task_name: &str,
    passthrough_args: &[String],
    registry: &AdapterRegistry,
) -> AppResult<Plan> {
    plan_workspace_filtered(workspace, task_name, passthrough_args, registry, None)
}

/// Build a task plan for modules included by `module_filter`.
pub fn plan_workspace_filtered(
    workspace: Workspace,
    task_name: &str,
    passthrough_args: &[String],
    registry: &AdapterRegistry,
    module_filter: Option<&BTreeSet<ScopedModuleKey>>,
) -> AppResult<Plan> {
    let discovered = discover_workspace_task_profiles(&workspace, task_name, registry)?;
    plan_discovered_task_profiles(workspace, &discovered, passthrough_args, module_filter)
}

/// Discover modules for every profile that defines `task_name`.
pub fn discover_workspace_task_profiles(
    workspace: &Workspace,
    task_name: &str,
    registry: &AdapterRegistry,
) -> AppResult<Vec<DiscoveredTaskProfile>> {
    let mut discovered = Vec::new();
    let mut discovery_cache = BTreeMap::new();

    for profile in &workspace.profiles {
        let config_profile_task = profile.tasks.iter().any(|task| task.name == task_name);
        let config_scope_task = profile
            .scope_overrides
            .iter()
            .any(|scope| scope.tasks.iter().any(|task| task.name == task_name));
        let adapter = match registry.adapter_for_profile(profile) {
            Ok(adapter) => adapter,
            Err(error) if config_profile_task || config_scope_task => return Err(error),
            Err(_) => continue,
        };
        let profile_tasks = merged_tasks(adapter.default_tasks(), profile.tasks.clone());
        let profile_task = profile_tasks.iter().find(|task| task.name == task_name);
        let scope_task_exists = profile.scope_overrides.iter().any(|scope| {
            scope.tasks.iter().any(|task| task.name == task_name) || profile_task.is_some()
        });
        if profile_task.is_none() && !scope_task_exists {
            continue;
        }
        let adapter_id = AdapterId::new(profile.language.clone())?;

        let profile_modules =
            discover_profile_modules(workspace, profile, registry, &mut discovery_cache)?;
        let mut scoped_modules = BTreeSet::new();

        for scope in &profile.scope_overrides {
            let scope_tasks = merged_tasks(profile_tasks.clone(), scope.tasks.clone());
            let Some(task) = scope_tasks.iter().find(|task| task.name == task_name) else {
                continue;
            };
            let mut modules =
                discover_scope_modules(workspace, profile, scope, registry, &mut discovery_cache)?;
            apply_profile_dependencies(&mut modules, &profile_modules);
            let scope_module_filter = modules
                .iter()
                .map(|module| module.name.clone())
                .collect::<BTreeSet<_>>();
            scoped_modules.extend(scope_module_filter.iter().cloned());
            discovered.push(DiscoveredTaskProfile {
                profile: scoped_profile(profile, scope),
                scope_id: ScopeId::new(scope.name.clone())?,
                adapter_id: adapter_id.clone(),
                task: task.clone(),
                modules,
            });
        }

        if let Some(task) = profile_task {
            let modules = profile_modules
                .into_iter()
                .filter(|module| !scoped_modules.contains(&module.name))
                .collect::<Vec<_>>();
            discovered.push(DiscoveredTaskProfile {
                profile: profile.clone(),
                scope_id: ScopeId::new(profile.name.clone())?,
                adapter_id,
                task: task.clone(),
                modules,
            });
        }
    }

    if discovered.is_empty() {
        return Err(AppError::invalid_input(
            "task",
            format!(
                "task '{task_name}' is not defined by any profile; available tasks: {}",
                available_tasks(workspace, registry)
            ),
        ));
    }

    Ok(discovered)
}

fn merged_tasks(base: Vec<Task>, overrides: Vec<Task>) -> Vec<Task> {
    let mut tasks = base
        .into_iter()
        .map(|task| (task.name.clone(), task))
        .collect::<BTreeMap<_, _>>();
    for task in overrides {
        tasks.insert(task.name.clone(), task);
    }
    tasks.into_values().collect()
}

fn apply_profile_dependencies(
    modules: &mut [crate::core::Module],
    profile_modules: &[crate::core::Module],
) {
    let dependencies_by_module = profile_modules
        .iter()
        .map(|module| (module.name.clone(), module.dependencies.clone()))
        .collect::<BTreeMap<_, _>>();

    for module in modules {
        if let Some(dependencies) = dependencies_by_module.get(&module.name) {
            module.dependencies.clone_from(dependencies);
        }
    }
}

/// Build a task plan from modules already discovered for the selected task.
pub fn plan_discovered_task_profiles(
    workspace: Workspace,
    discovered: &[DiscoveredTaskProfile],
    passthrough_args: &[String],
    module_filter: Option<&BTreeSet<ScopedModuleKey>>,
) -> AppResult<Plan> {
    let mut units = Vec::new();
    let mut grouped = BTreeMap::<String, Vec<&DiscoveredTaskProfile>>::new();

    for discovered in discovered {
        grouped
            .entry(discovered.profile.name.clone())
            .or_default()
            .push(discovered);
    }

    for group in grouped.into_values() {
        units.extend(plan_discovered_profile_group(
            &workspace,
            &group,
            passthrough_args,
            module_filter,
        )?);
    }

    Ok(Plan { workspace, units })
}

fn plan_discovered_profile_group(
    workspace: &Workspace,
    discovered: &[&DiscoveredTaskProfile],
    passthrough_args: &[String],
    module_filter: Option<&BTreeSet<ScopedModuleKey>>,
) -> AppResult<Vec<ExecutionUnit>> {
    let mut modules_by_key = BTreeMap::new();
    let mut all_modules_by_key = BTreeMap::new();
    let mut policy_by_module = BTreeMap::new();
    let mut all_policy_modules = BTreeMap::<usize, Vec<crate::core::Module>>::new();

    for (policy_index, policy) in discovered.iter().enumerate() {
        for module in policy.modules.clone() {
            let module_key = scoped_module_key(&module);
            if all_modules_by_key
                .insert(module_key.clone(), module.clone())
                .is_some()
            {
                return Err(AppError::invalid_input(
                    "modules",
                    format!("duplicate module '{}/{}'", module.scope_id, module.name),
                ));
            }

            if !module_matches_filter(&module, module_filter) {
                continue;
            }

            modules_by_key.insert(module_key.clone(), module.clone());
            policy_by_module.insert(module_key, policy_index);
            all_policy_modules
                .entry(policy_index)
                .or_default()
                .push(module);
        }
    }

    let all_discovered_modules = all_modules_by_key.into_values().collect::<Vec<_>>();
    let full_graph =
        resolve_dependency_graph(&all_discovered_modules, &workspace.dependency_overlays)?;
    let all_modules = modules_by_key.into_values().collect::<Vec<_>>();
    let selected_keys = all_modules
        .iter()
        .map(scoped_module_key)
        .collect::<BTreeSet<_>>();

    let mut units = Vec::new();
    for (policy_index, modules) in &all_policy_modules {
        let policy = discovered[*policy_index];
        if policy.profile.execution == ExecutionMode::WorkspaceOnce && !modules.is_empty() {
            units.push(unit(
                &policy.profile,
                PlanIdentity::new(&policy.scope_id, &policy.adapter_id),
                &policy.task,
                format!(
                    "{}/workspace",
                    unit_id_prefix(&policy.scope_id, &policy.task)
                ),
                modules.clone(),
                task_command(&policy.task)?,
                passthrough_args.to_owned(),
            ));
        }
    }

    for (wave_index, wave) in
        scoped_ready_waves_with_graph(all_modules, &full_graph, &selected_keys)?
            .into_iter()
            .enumerate()
    {
        let mut wave_by_policy = BTreeMap::<usize, Vec<crate::core::Module>>::new();
        for module in wave {
            let module_key = scoped_module_key(&module);
            let policy_index = policy_by_module.get(&module_key).ok_or_else(|| {
                AppError::invalid_input(
                    "modules",
                    format!(
                        "missing planning policy for module '{}/{}'",
                        module.scope_id, module.name
                    ),
                )
            })?;
            wave_by_policy
                .entry(*policy_index)
                .or_default()
                .push(module);
        }

        for (policy_index, modules) in wave_by_policy {
            let policy = discovered[policy_index];
            if policy.profile.execution == ExecutionMode::WorkspaceOnce {
                continue;
            }
            units.extend(plan_ready_wave(
                &policy.profile,
                PlanIdentity::new(&policy.scope_id, &policy.adapter_id),
                &policy.task,
                wave_index,
                modules,
                passthrough_args,
            )?);
        }
    }

    for unit in &units {
        render_execution_unit(unit, &workspace.root)?;
        render_resource_group(unit, &workspace.root)?;
    }

    Ok(units)
}

fn module_matches_filter(
    module: &crate::core::Module,
    module_filter: Option<&BTreeSet<ScopedModuleKey>>,
) -> bool {
    let Some(module_filter) = module_filter else {
        return true;
    };
    module_filter.contains(&scoped_module_key(module))
}

#[cfg(test)]
fn scoped_ready_waves(
    modules: Vec<crate::core::Module>,
    overlays: &[crate::core::DependencyOverlay],
) -> AppResult<Vec<Vec<crate::core::Module>>> {
    let modules_by_key = modules
        .into_iter()
        .map(|module| (scoped_module_key(&module), module))
        .collect::<BTreeMap<_, _>>();
    let selected_keys = modules_by_key.keys().cloned().collect::<BTreeSet<_>>();
    let graph = crate::engine::graph::resolve_selected_dependency_graph(
        &modules_by_key.values().cloned().collect::<Vec<_>>(),
        overlays,
    )?;

    scoped_ready_waves_with_graph(
        modules_by_key.into_values().collect(),
        &graph,
        &selected_keys,
    )
}

fn scoped_ready_waves_with_graph(
    modules: Vec<crate::core::Module>,
    graph: &ResolvedDependencyGraph,
    selected_keys: &BTreeSet<ScopedModuleKey>,
) -> AppResult<Vec<Vec<crate::core::Module>>> {
    let modules_by_key = modules
        .into_iter()
        .map(|module| (scoped_module_key(&module), module))
        .collect::<BTreeMap<_, _>>();
    let mut remaining = BTreeMap::<ScopedModuleKey, usize>::new();

    for key in modules_by_key.keys() {
        let dependency_count = graph
            .dependencies(key)
            .into_iter()
            .filter(|dependency| selected_keys.contains(dependency))
            .count();
        remaining.insert(key.clone(), dependency_count);
    }

    let mut ready = remaining
        .iter()
        .filter_map(|(key, count)| (*count == 0).then_some(key.clone()))
        .collect::<BTreeSet<_>>();
    let mut satisfied = BTreeSet::new();
    let mut waves = Vec::new();

    while !ready.is_empty() {
        let current = std::mem::take(&mut ready);
        let mut wave = Vec::with_capacity(current.len());
        for key in &current {
            satisfied.insert(key.clone());
            remaining.remove(key);
            if let Some(module) = modules_by_key.get(key) {
                wave.push(module.clone());
            }
        }
        waves.push(wave);

        for key in current {
            {
                for next in graph.dependents(&key) {
                    if satisfied.contains(next) {
                        continue;
                    }
                    let Some(count) = remaining.get_mut(next) else {
                        continue;
                    };
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        ready.insert(next.clone());
                    }
                }
            }
        }
    }

    if !remaining.is_empty() {
        let modules = remaining
            .keys()
            .map(|(scope_id, module)| format!("{scope_id}/{module}"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(AppError::invalid_input(
            "modules",
            format!("module dependency cycle detected among: {modules}"),
        ));
    }

    Ok(waves)
}

fn discover_profile_modules(
    workspace: &Workspace,
    profile: &Profile,
    registry: &AdapterRegistry,
    cache: &mut BTreeMap<String, Vec<crate::core::Module>>,
) -> AppResult<Vec<crate::core::Module>> {
    discover_modules(
        workspace,
        profile,
        ScopeId::new(profile.name.clone())?,
        profile.adapter_options.clone(),
        registry,
        cache,
    )
}

fn discover_scope_modules(
    workspace: &Workspace,
    profile: &Profile,
    scope: &ScopeOverride,
    registry: &AdapterRegistry,
    cache: &mut BTreeMap<String, Vec<crate::core::Module>>,
) -> AppResult<Vec<crate::core::Module>> {
    let adapter_options = if scope.adapter_options.is_empty() {
        profile.adapter_options.clone()
    } else {
        scope.adapter_options.clone()
    };
    discover_modules(
        workspace,
        profile,
        ScopeId::new(scope.name.clone())?,
        adapter_options,
        registry,
        cache,
    )
}

fn discover_modules(
    workspace: &Workspace,
    profile: &Profile,
    scope_id: ScopeId,
    adapter_options: crate::core::AdapterOptions,
    registry: &AdapterRegistry,
    cache: &mut BTreeMap<String, Vec<crate::core::Module>>,
) -> AppResult<Vec<crate::core::Module>> {
    let adapter_id = AdapterId::new(profile.language.clone())?;
    let adapter_options_key =
        serde_json::to_string(&adapter_options).map_err(AppError::internal)?;
    let discovery_command_key =
        serde_json::to_string(&profile.discovery_command).map_err(AppError::internal)?;
    let cache_key =
        format!("{scope_id}:{adapter_id}:{discovery_command_key}:{adapter_options_key}");
    if let Some(modules) = cache.get(&cache_key) {
        return Ok(modules.clone());
    }

    let adapter = registry.adapter_for_profile(profile)?;
    let request = DiscoverRequest {
        schema_version: DISCOVERY_SCHEMA_VERSION,
        project_root: workspace.root.clone(),
        scope_id,
        adapter_id,
        scope_root: std::path::PathBuf::from("."),
        adapter_options,
    };
    let response = adapter.discover(&request)?;
    validate_discovery_response(
        format!("profiles.{}.adapter", profile.name),
        &request,
        &response,
    )?;
    let modules = response
        .modules
        .into_iter()
        .map(crate::core::DiscoveredModule::into_module)
        .collect::<Vec<_>>();
    cache.insert(cache_key, modules.clone());
    Ok(modules)
}

fn scoped_profile(profile: &Profile, scope: &ScopeOverride) -> Profile {
    let mut scoped = profile.clone();
    scoped.execution = scope.execution.unwrap_or(profile.execution);
    if let Some(module_arg_template) = &scope.module_arg_template {
        scoped.module_arg_template.clone_from(module_arg_template);
    }
    if let Some(resource_group) = &scope.resource_group {
        scoped.resource_group.clone_from(resource_group);
    }
    scoped.scope_overrides = Vec::new();
    scoped
}

#[cfg(test)]
fn plan_profile_task(
    workspace: &Workspace,
    profile: &Profile,
    scope: Option<&str>,
    task: &Task,
    modules: Vec<crate::core::Module>,
    passthrough_args: &[String],
) -> AppResult<Vec<ExecutionUnit>> {
    let mut units = Vec::new();
    let scope_id = ScopeId::new(scope.unwrap_or(&profile.name))?;
    let adapter_id = AdapterId::new(profile.language.clone())?;

    if modules.is_empty() {
        return Ok(units);
    }

    let command = task_command(task)?;
    let waves = scoped_ready_waves(modules.clone(), &[])?;

    match profile.execution {
        ExecutionMode::SpawnEach | ExecutionMode::BatchReady => {
            for (wave_index, wave) in waves.into_iter().enumerate() {
                units.extend(plan_ready_wave(
                    profile,
                    PlanIdentity::new(&scope_id, &adapter_id),
                    task,
                    wave_index,
                    wave,
                    passthrough_args,
                )?);
            }
        }
        ExecutionMode::WorkspaceOnce => {
            units.push(unit(
                profile,
                PlanIdentity::new(&scope_id, &adapter_id),
                task,
                format!("{}/workspace", unit_id_prefix(&scope_id, task)),
                modules,
                command,
                passthrough_args.to_owned(),
            ));
        }
    }

    for unit in &units {
        render_execution_unit(unit, &workspace.root)?;
        render_resource_group(unit, &workspace.root)?;
    }
    Ok(units)
}

fn plan_ready_wave(
    profile: &Profile,
    identity: PlanIdentity<'_>,
    task: &Task,
    wave_index: usize,
    modules: Vec<crate::core::Module>,
    passthrough_args: &[String],
) -> AppResult<Vec<ExecutionUnit>> {
    let command = task_command(task)?;
    let mut units = Vec::new();

    match profile.execution {
        ExecutionMode::SpawnEach => {
            for module in modules {
                units.push(unit(
                    profile,
                    identity,
                    task,
                    format!(
                        "{}/w{wave_index}/{}",
                        unit_id_prefix(identity.scope_id, task),
                        module.name
                    ),
                    vec![module],
                    command.clone(),
                    passthrough_args.to_owned(),
                ));
            }
        }
        ExecutionMode::BatchReady => {
            let groups = split_wave_by_manifest(modules);
            let split = groups.len() > 1;
            for (group_index, group) in groups.into_iter().enumerate() {
                let id = if split {
                    format!(
                        "{}/w{wave_index}/batch/m{group_index}",
                        unit_id_prefix(identity.scope_id, task)
                    )
                } else {
                    format!(
                        "{}/w{wave_index}/batch",
                        unit_id_prefix(identity.scope_id, task)
                    )
                };
                units.push(unit(
                    profile,
                    identity,
                    task,
                    id,
                    group,
                    command.clone(),
                    passthrough_args.to_owned(),
                ));
            }
        }
        ExecutionMode::WorkspaceOnce => {}
    }

    Ok(units)
}

#[derive(Clone, Copy)]
struct PlanIdentity<'a> {
    scope_id: &'a ScopeId,
    adapter_id: &'a AdapterId,
}

impl<'a> PlanIdentity<'a> {
    const fn new(scope_id: &'a ScopeId, adapter_id: &'a AdapterId) -> Self {
        Self {
            scope_id,
            adapter_id,
        }
    }
}

fn unit_id_prefix(scope_id: &ScopeId, task: &Task) -> String {
    format!("{scope_id}/{}", task.name)
}

fn unit(
    profile: &Profile,
    identity: PlanIdentity<'_>,
    task: &Task,
    id: String,
    modules: Vec<crate::core::Module>,
    command: PlannedCommand,
    passthrough_args: Vec<String>,
) -> ExecutionUnit {
    ExecutionUnit {
        id,
        scope_id: identity.scope_id.clone(),
        adapter_id: identity.adapter_id.clone(),
        task: task.name.clone(),
        command_origin: command.origin,
        task_origin: task.origin.clone(),
        mode: profile.execution,
        resource_group: profile.resource_group.clone(),
        modules,
        argv_template: command.argv_template,
        module_arg_template: profile.module_arg_template.clone(),
        passthrough_args,
        cache_args: task.cache_args,
        persistent: task.persistent,
        readiness: task.readiness.clone(),
        readiness_timeout: task.readiness_timeout,
        shared_inputs: command.shared_inputs,
    }
}

#[derive(Clone)]
struct PlannedCommand {
    argv_template: Vec<String>,
    origin: CommandOrigin,
    shared_inputs: Vec<String>,
}

fn task_command(task: &Task) -> AppResult<PlannedCommand> {
    match &task.command {
        TaskCommand::Argv(argv) => Ok(PlannedCommand {
            argv_template: argv.clone(),
            origin: CommandOrigin::DirectArgv,
            shared_inputs: task.shared_inputs.clone(),
        }),
        TaskCommand::ResolvedPreset(preset) => {
            let mut shared_inputs = preset.shared_inputs.clone();
            shared_inputs.extend(task.shared_inputs.clone());
            Ok(PlannedCommand {
                argv_template: preset.argv.clone(),
                origin: CommandOrigin::Preset {
                    name: preset.name.clone(),
                    language: preset.language.clone(),
                },
                shared_inputs,
            })
        }
        TaskCommand::Preset(name) => Err(AppError::invalid_input(
            "task",
            format!("task preset '{name}' was not resolved"),
        )),
    }
}

fn available_tasks(workspace: &Workspace, registry: &AdapterRegistry) -> String {
    workspace
        .profiles
        .iter()
        .map(|profile| {
            let default_tasks = registry
                .adapter_for_profile(profile)
                .map(|adapter| adapter.default_tasks())
                .unwrap_or_default();
            let tasks = merged_tasks(default_tasks, profile.tasks.clone())
                .into_iter()
                .map(|task| task.name)
                .collect::<Vec<_>>()
                .join(", ");
            format!("{} [{}]", profile.name, tasks)
        })
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, path::PathBuf};

    use crate::{
        adapter::AdapterRegistry,
        config::load_workspace,
        core::{
            AdapterId, DependencyOverlay, ExecutionMode, Module, ModuleId, PersistentReadiness,
            PresetDefinition, Profile, ScopeId, Task, TaskCommand, TaskOrigin, Workspace,
        },
        engine::planner::{
            DiscoveredTaskProfile, plan_discovered_task_profiles, plan_profile_task,
            plan_workspace, scoped_ready_waves,
        },
        exec::render_resource_group,
    };

    fn module(name: &str) -> Module {
        module_with_manifest(name, "Cargo.toml")
    }

    fn module_with_manifest(name: &str, manifest: &str) -> Module {
        module_in_scope("rust", name, manifest)
    }

    fn module_in_scope(scope: &str, name: &str, manifest: &str) -> Module {
        Module {
            scope_id: ScopeId::new(scope).expect("scope id"),
            adapter_id: AdapterId::new("rust").expect("adapter id"),
            name: ModuleId::new(name).expect("module id"),
            package: Some(format!("{name}-pkg")),
            root: PathBuf::from(name),
            manifest: Some(PathBuf::from(manifest)),
            dependencies: Vec::new(),
            source_patterns: Vec::new(),
        }
    }

    fn task() -> Task {
        Task {
            name: "test".to_string(),
            command: TaskCommand::Argv(vec!["cargo".to_string(), "test".to_string()]),
            origin: TaskOrigin::ProjectDefault,
            cache_args: false,
            shared_inputs: Vec::new(),
            persistent: false,
            readiness: PersistentReadiness::Started,
            readiness_timeout: std::time::Duration::from_secs(30),
        }
    }

    fn profile(execution: ExecutionMode) -> Profile {
        Profile {
            name: "rust".to_string(),
            language: "rust".to_string(),
            adapter_options: std::collections::BTreeMap::default(),
            discovery_command: None,
            execution,
            module_arg_template: Vec::new(),
            resource_group: "cargo:{project.root}".to_string(),
            tasks: vec![task()],
            scope_overrides: Vec::new(),
        }
    }

    #[test]
    fn validates_resource_group_during_planning() {
        let workspace = Workspace {
            schema: 1,
            name: "fixture".to_string(),
            root: PathBuf::from("/workspace"),
            base_ref: None,
            profiles: Vec::new(),
            dependency_overlays: Vec::new(),
        };
        let profile = Profile {
            name: "rust".to_string(),
            language: "rust".to_string(),
            adapter_options: std::collections::BTreeMap::default(),
            discovery_command: None,
            execution: ExecutionMode::BatchReady,
            module_arg_template: Vec::new(),
            resource_group: "cargo:{module.package}".to_string(),
            tasks: vec![task()],
            scope_overrides: Vec::new(),
        };

        let error = plan_profile_task(
            &workspace,
            &profile,
            None,
            &profile.tasks[0],
            vec![module("core"), module("app")],
            &[],
        )
        .expect_err("invalid resource group should fail during planning");

        assert!(error.message.contains("resource_group"));
    }

    #[test]
    fn does_not_emit_workspace_unit_for_empty_module_set() {
        let workspace = Workspace {
            schema: 1,
            name: "fixture".to_string(),
            root: PathBuf::from("/workspace"),
            base_ref: None,
            profiles: Vec::new(),
            dependency_overlays: Vec::new(),
        };
        let profile = Profile {
            name: "rust".to_string(),
            language: "rust".to_string(),
            adapter_options: std::collections::BTreeMap::default(),
            discovery_command: None,
            execution: ExecutionMode::WorkspaceOnce,
            module_arg_template: Vec::new(),
            resource_group: "cargo:{project.root}".to_string(),
            tasks: vec![task()],
            scope_overrides: Vec::new(),
        };

        let units = plan_profile_task(
            &workspace,
            &profile,
            None,
            &profile.tasks[0],
            Vec::new(),
            &[],
        )
        .expect("empty planning succeeds");

        assert!(units.is_empty());
    }

    #[test]
    fn splits_batch_ready_waves_by_manifest() {
        let workspace = Workspace {
            schema: 1,
            name: "fixture".to_string(),
            root: PathBuf::from("/workspace"),
            base_ref: None,
            profiles: Vec::new(),
            dependency_overlays: Vec::new(),
        };
        let profile = Profile {
            name: "rust".to_string(),
            language: "rust".to_string(),
            adapter_options: std::collections::BTreeMap::default(),
            discovery_command: None,
            execution: ExecutionMode::BatchReady,
            module_arg_template: Vec::new(),
            resource_group: "cargo:{project.root}".to_string(),
            tasks: vec![task()],
            scope_overrides: Vec::new(),
        };

        let units = plan_profile_task(
            &workspace,
            &profile,
            None,
            &profile.tasks[0],
            vec![
                module_with_manifest("core", "core/Cargo.toml"),
                module_with_manifest("contrib", "contrib/Cargo.toml"),
            ],
            &[],
        )
        .expect("planning succeeds");

        assert_eq!(units.len(), 2);
        assert!(units.iter().all(|unit| unit.modules.len() == 1));
    }

    #[test]
    fn task_shared_inputs_extend_preset_shared_inputs() {
        let workspace = Workspace {
            schema: 1,
            name: "fixture".to_string(),
            root: PathBuf::from("/workspace"),
            base_ref: None,
            profiles: Vec::new(),
            dependency_overlays: Vec::new(),
        };
        let task = Task {
            name: "test".to_string(),
            command: TaskCommand::ResolvedPreset(PresetDefinition {
                name: "nextest".to_string(),
                language: "rust".to_string(),
                argv: vec!["cargo".to_string(), "nextest".to_string()],
                shared_inputs: vec!["Cargo.lock".to_string()],
            }),
            origin: TaskOrigin::ProjectDefault,
            cache_args: false,
            shared_inputs: vec!["rust-toolchain.toml".to_string()],
            persistent: false,
            readiness: PersistentReadiness::Started,
            readiness_timeout: std::time::Duration::from_secs(30),
        };
        let profile = Profile {
            name: "rust".to_string(),
            language: "rust".to_string(),
            adapter_options: std::collections::BTreeMap::default(),
            discovery_command: None,
            execution: ExecutionMode::BatchReady,
            module_arg_template: Vec::new(),
            resource_group: "cargo:{project.root}".to_string(),
            tasks: vec![task],
            scope_overrides: Vec::new(),
        };

        let units = plan_profile_task(
            &workspace,
            &profile,
            None,
            &profile.tasks[0],
            vec![module("core")],
            &[],
        )
        .expect("planning succeeds");

        assert_eq!(
            units[0].shared_inputs,
            ["Cargo.lock", "rust-toolchain.toml"]
        );
    }

    #[test]
    fn schedules_duplicate_module_names_across_scopes() {
        let waves = scoped_ready_waves(
            vec![
                module_in_scope("base", "shared", "Cargo.toml"),
                module_in_scope("override", "shared", "Cargo.toml"),
            ],
            &[],
        )
        .expect("duplicate module names in separate scopes schedule");

        let scheduled = waves
            .into_iter()
            .flatten()
            .map(|module| format!("{}/{}", module.scope_id, module.name))
            .collect::<Vec<_>>();
        assert_eq!(scheduled, ["base/shared", "override/shared"]);
    }

    #[test]
    fn releases_scope_aware_dependency_waves_in_order() {
        let mut app = module_in_scope("rust", "app", "Cargo.toml");
        app.dependencies = vec![ModuleId::new("core").expect("module id")];
        let waves = scoped_ready_waves(
            vec![app, module_in_scope("rust", "core", "Cargo.toml")],
            &[],
        )
        .expect("waves schedule");

        let names = waves
            .into_iter()
            .map(|wave| {
                wave.into_iter()
                    .map(|module| format!("{}/{}", module.scope_id, module.name))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        assert_eq!(names, [vec!["rust/core"], vec!["rust/app"]]);
    }

    #[test]
    fn releases_overlay_dependency_waves_in_order() {
        let overlays = [DependencyOverlay {
            from: ("app".to_string(), ModuleId::new("api").expect("module id")),
            to: (
                "lib".to_string(),
                ModuleId::new("shared").expect("module id"),
            ),
        }];
        let waves = scoped_ready_waves(
            vec![
                module_in_scope("app", "api", "app/Cargo.toml"),
                module_in_scope("lib", "shared", "lib/Cargo.toml"),
            ],
            &overlays,
        )
        .expect("waves schedule");

        let names = waves
            .into_iter()
            .map(|wave| {
                wave.into_iter()
                    .map(|module| format!("{}/{}", module.scope_id, module.name))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        assert_eq!(names, [vec!["lib/shared"], vec!["app/api"]]);
    }

    #[test]
    fn filtered_plan_resolves_dependencies_before_dropping_unselected_modules() {
        let workspace = Workspace {
            schema: 1,
            name: "fixture".to_string(),
            root: PathBuf::from("/workspace"),
            base_ref: None,
            profiles: Vec::new(),
            dependency_overlays: Vec::new(),
        };
        let profile = profile(ExecutionMode::SpawnEach);
        let mut api = module_in_scope("app", "api", "app/Cargo.toml");
        api.dependencies = vec![ModuleId::new("shared").expect("module id")];
        let discovered = [
            DiscoveredTaskProfile {
                profile: profile.clone(),
                scope_id: ScopeId::new("app").expect("scope id"),
                adapter_id: AdapterId::new("rust").expect("adapter id"),
                task: profile.tasks[0].clone(),
                modules: vec![api, module_in_scope("app", "shared", "app/Cargo.toml")],
            },
            DiscoveredTaskProfile {
                profile: profile.clone(),
                scope_id: ScopeId::new("lib").expect("scope id"),
                adapter_id: AdapterId::new("rust").expect("adapter id"),
                task: profile.tasks[0].clone(),
                modules: vec![module_in_scope("lib", "shared", "lib/Cargo.toml")],
            },
        ];
        let filter = BTreeSet::from([
            ("app".to_string(), ModuleId::new("api").expect("module id")),
            (
                "lib".to_string(),
                ModuleId::new("shared").expect("module id"),
            ),
        ]);

        let plan = plan_discovered_task_profiles(workspace, &discovered, &[], Some(&filter))
            .expect("filtered plan succeeds");
        let unit_ids = plan
            .units
            .iter()
            .map(|unit| unit.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(unit_ids, ["app/test/w0/api", "lib/test/w0/shared"]);
    }

    #[test]
    fn rejects_scope_aware_cycles() {
        let mut left = module_in_scope("rust", "left", "Cargo.toml");
        left.dependencies = vec![ModuleId::new("right").expect("module id")];
        let mut right = module_in_scope("rust", "right", "Cargo.toml");
        right.dependencies = vec![ModuleId::new("left").expect("module id")];

        let error = scoped_ready_waves(vec![left, right], &[]).expect_err("cycle should fail");

        assert!(error.message.contains("cycle"));
        assert!(error.message.contains("rust/left"));
    }

    #[test]
    fn applies_scope_task_overrides_to_discovered_partition() {
        let root = rskit_testutil::test_workspace!("scope-plan");
        let workspace_path = root.path().join("project");
        rskit_fs::sync_io::tree::copy_tree(
            &root
                .fixture_path("rust-cross-workspaces")
                .expect("rust fixture path"),
            &workspace_path,
            rskit_fs::sync_io::tree::CopyTreeOptions::default(),
        )
        .expect("copy rust fixture");

        let workspace = load_workspace(workspace_path.join("toven.toml")).expect("config loads");
        let plan = plan_workspace(workspace, "test", &[], &AdapterRegistry::default())
            .expect("plan succeeds");

        let scoped = plan
            .units
            .iter()
            .find(|unit| unit.scope_id.as_str() == "contrib")
            .expect("contrib scope unit exists");
        assert!(scoped.id.starts_with("contrib/test/"));
        assert_eq!(scoped.modules[0].name.as_str(), "contrib-app");
        assert_eq!(scoped.argv_template[1], "check");
        assert_eq!(
            scoped.task_origin,
            TaskOrigin::ScopeOverride {
                scope_id: ScopeId::new("contrib").expect("scope id")
            }
        );
        assert_eq!(scoped.mode, ExecutionMode::SpawnEach);
        assert_eq!(
            render_resource_group(scoped, &plan.workspace.root).expect("resource group renders"),
            "scope:.:contrib-app"
        );

        let base_modules = plan
            .units
            .iter()
            .filter(|unit| unit.scope_id.as_str() == "rust")
            .flat_map(|unit| {
                unit.modules
                    .iter()
                    .map(|module| module.name.as_str())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        assert_eq!(base_modules, ["core-local"]);

        let unit_modules = plan
            .units
            .iter()
            .map(|unit| {
                unit.modules
                    .iter()
                    .map(|module| module.name.as_str())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        assert_eq!(unit_modules, [vec!["core-local"], vec!["contrib-app"]]);
    }

    #[test]
    fn skips_profiles_that_cannot_run_requested_task_before_discovery() {
        let root = rskit_testutil::test_workspace!("skip-unrelated-profile");
        let workspace_path = root.path().join("project");
        rskit_fs::sync_io::tree::copy_tree(
            &root
                .fixture_path("rust-cross-workspaces")
                .expect("rust fixture path"),
            &workspace_path,
            rskit_fs::sync_io::tree::CopyTreeOptions::default(),
        )
        .expect("copy rust fixture");

        let mut workspace =
            load_workspace(workspace_path.join("toven.toml")).expect("config loads");
        workspace.profiles.push(Profile {
            name: "unused".to_string(),
            language: "unsupported".to_string(),
            adapter_options: std::collections::BTreeMap::default(),
            discovery_command: None,
            execution: ExecutionMode::SpawnEach,
            module_arg_template: Vec::new(),
            resource_group: "{project.root}".to_string(),
            tasks: Vec::new(),
            scope_overrides: Vec::new(),
        });

        plan_workspace(workspace, "test", &[], &AdapterRegistry::default())
            .expect("unrelated profile without task is skipped");
    }

    #[test]
    fn plans_rust_adapter_default_tasks_without_project_task_config() {
        let root = rskit_testutil::test_workspace!("rust-adapter-defaults");
        let workspace_path = root.path().join("project");
        rskit_fs::sync_io::tree::copy_tree(
            &root
                .fixture_path("rust-workspace")
                .expect("rust fixture path"),
            &workspace_path,
            rskit_fs::sync_io::tree::CopyTreeOptions::default(),
        )
        .expect("copy rust fixture");
        std::fs::copy(
            root.fixture_path("config/rust-adapter-defaults.toml")
                .expect("default config fixture"),
            workspace_path.join("toven.toml"),
        )
        .expect("copy default config");

        let workspace = load_workspace(workspace_path.join("toven.toml")).expect("config loads");
        let plan = plan_workspace(workspace, "check", &[], &AdapterRegistry::default())
            .expect("adapter default task plans");

        assert!(!plan.units.is_empty());
        assert!(plan.units.iter().all(|unit| {
            unit.scope_id.as_str() == "rust"
                && unit.adapter_id.as_str() == "rust"
                && unit.task_origin
                    == TaskOrigin::AdapterDefault {
                        adapter_id: AdapterId::new("rust").expect("adapter id"),
                    }
                && unit.argv_template
                    == [
                        "cargo",
                        "check",
                        "--manifest-path",
                        "{module.manifest}",
                        "-p",
                        "{module.package}",
                        "{args}",
                    ]
        }));
    }
}
