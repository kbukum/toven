//! Execution unit planning.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    adapter::AdapterRegistry,
    core::{
        AdapterId, AppError, AppResult, CommandOrigin, DISCOVERY_SCHEMA_VERSION, DiscoverRequest,
        ExecutionMode, ExecutionUnit, ModuleId, Plan, Profile, ScopeId, ScopeOverride, Task,
        TaskCommand, Workspace, validate_discovery_response,
    },
    engine::scheduler::{ready_waves, split_wave_by_manifest},
    exec::{render_execution_unit, render_resource_group},
};

/// Modules discovered for one profile/task pair.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DiscoveredTaskProfile {
    /// Profile that owns the task.
    pub profile: Profile,
    /// Scope override that owns this planned partition.
    pub scope: Option<String>,
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
    module_filter: Option<&BTreeSet<ModuleId>>,
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
        let profile_task = profile.tasks.iter().find(|task| task.name == task_name);
        let scope_task_exists = profile.scope_overrides.iter().any(|scope| {
            scope.tasks.iter().any(|task| task.name == task_name) || profile_task.is_some()
        });
        if profile_task.is_none() && !scope_task_exists {
            continue;
        }

        let mut scoped_modules = BTreeSet::new();

        for scope in &profile.scope_overrides {
            let Some(task) = scope
                .tasks
                .iter()
                .find(|task| task.name == task_name)
                .or(profile_task)
            else {
                continue;
            };
            let modules =
                discover_scope_modules(workspace, profile, scope, registry, &mut discovery_cache)?;
            let scope_module_filter = modules
                .iter()
                .map(|module| module.name.clone())
                .collect::<BTreeSet<_>>();
            scoped_modules.extend(scope_module_filter.iter().cloned());
            discovered.push(DiscoveredTaskProfile {
                profile: scoped_profile(profile, scope),
                scope: Some(scope.name.clone()),
                task: task.clone(),
                modules,
            });
        }

        if let Some(task) = profile_task {
            let profile_modules =
                discover_profile_modules(workspace, profile, registry, &mut discovery_cache)?;
            let modules = profile_modules
                .into_iter()
                .filter(|module| !scoped_modules.contains(&module.name))
                .collect::<Vec<_>>();
            discovered.push(DiscoveredTaskProfile {
                profile: profile.clone(),
                scope: None,
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
                available_tasks(workspace)
            ),
        ));
    }

    Ok(discovered)
}

/// Build a task plan from modules already discovered for the selected task.
pub fn plan_discovered_task_profiles(
    workspace: Workspace,
    discovered: &[DiscoveredTaskProfile],
    passthrough_args: &[String],
    module_filter: Option<&BTreeSet<ModuleId>>,
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
    module_filter: Option<&BTreeSet<ModuleId>>,
) -> AppResult<Vec<ExecutionUnit>> {
    let mut modules_by_name = BTreeMap::new();
    let mut policy_by_module = BTreeMap::new();
    let mut all_policy_modules = BTreeMap::<usize, Vec<crate::core::Module>>::new();

    for (policy_index, policy) in discovered.iter().enumerate() {
        for module in filter_modules(policy.modules.clone(), module_filter) {
            if modules_by_name
                .insert(module.name.clone(), module.clone())
                .is_some()
            {
                return Err(AppError::invalid_input(
                    "modules",
                    format!("duplicate module '{}'", module.name),
                ));
            }
            policy_by_module.insert(module.name.clone(), policy_index);
            all_policy_modules
                .entry(policy_index)
                .or_default()
                .push(module);
        }
    }

    let selected_names = modules_by_name.keys().cloned().collect::<BTreeSet<_>>();
    let mut all_modules = modules_by_name.into_values().collect::<Vec<_>>();
    for module in &mut all_modules {
        module
            .dependencies
            .retain(|dependency| selected_names.contains(dependency));
    }

    let mut units = Vec::new();
    for (policy_index, modules) in &all_policy_modules {
        let policy = discovered[*policy_index];
        if policy.profile.execution == ExecutionMode::WorkspaceOnce && !modules.is_empty() {
            units.push(unit(
                &policy.profile,
                policy.scope.as_deref(),
                &policy.task,
                format!(
                    "{}/workspace",
                    unit_id_prefix(&policy.profile, policy.scope.as_deref(), &policy.task)
                ),
                modules.clone(),
                task_command(&policy.task)?,
                passthrough_args.to_owned(),
            ));
        }
    }

    for (wave_index, wave) in ready_waves(&all_modules)?.into_iter().enumerate() {
        let mut wave_by_policy = BTreeMap::<usize, Vec<crate::core::Module>>::new();
        for module in wave {
            let policy_index = policy_by_module.get(&module.name).ok_or_else(|| {
                AppError::invalid_input(
                    "modules",
                    format!("missing planning policy for module '{}'", module.name),
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
                policy.scope.as_deref(),
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

fn filter_modules(
    modules: Vec<crate::core::Module>,
    module_filter: Option<&BTreeSet<ModuleId>>,
) -> Vec<crate::core::Module> {
    let Some(module_filter) = module_filter else {
        return modules;
    };
    modules
        .into_iter()
        .filter_map(|mut module| {
            if !module_filter.contains(&module.name) {
                return None;
            }
            module
                .dependencies
                .retain(|dependency| module_filter.contains(dependency));
            Some(module)
        })
        .collect()
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

    if modules.is_empty() {
        return Ok(units);
    }

    let command = task_command(task)?;
    let waves = ready_waves(&modules)?;

    match profile.execution {
        ExecutionMode::SpawnEach | ExecutionMode::BatchReady => {
            for (wave_index, wave) in waves.into_iter().enumerate() {
                units.extend(plan_ready_wave(
                    profile,
                    scope,
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
                scope,
                task,
                format!("{}/workspace", unit_id_prefix(profile, scope, task)),
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
    scope: Option<&str>,
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
                    scope,
                    task,
                    format!(
                        "{}/w{wave_index}/{}",
                        unit_id_prefix(profile, scope, task),
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
                        unit_id_prefix(profile, scope, task)
                    )
                } else {
                    format!(
                        "{}/w{wave_index}/batch",
                        unit_id_prefix(profile, scope, task)
                    )
                };
                units.push(unit(
                    profile,
                    scope,
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

fn unit_id_prefix(profile: &Profile, scope: Option<&str>, task: &Task) -> String {
    scope.map_or_else(
        || format!("{}/{}", profile.name, task.name),
        |scope| format!("{}/{}/{}", profile.name, scope, task.name),
    )
}

fn unit(
    profile: &Profile,
    scope: Option<&str>,
    task: &Task,
    id: String,
    modules: Vec<crate::core::Module>,
    command: PlannedCommand,
    passthrough_args: Vec<String>,
) -> ExecutionUnit {
    ExecutionUnit {
        id,
        profile: profile.name.clone(),
        scope: scope.map(ToString::to_string),
        task: task.name.clone(),
        command_origin: command.origin,
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
            shared_inputs: Vec::new(),
        }),
        TaskCommand::ResolvedPreset(preset) => Ok(PlannedCommand {
            argv_template: preset.argv.clone(),
            origin: CommandOrigin::Preset {
                name: preset.name.clone(),
                language: preset.language.clone(),
            },
            shared_inputs: preset.shared_inputs.clone(),
        }),
        TaskCommand::Preset(name) => Err(AppError::invalid_input(
            "task",
            format!("task preset '{name}' was not resolved"),
        )),
    }
}

fn available_tasks(workspace: &Workspace) -> String {
    workspace
        .profiles
        .iter()
        .map(|profile| {
            let tasks = profile
                .tasks
                .iter()
                .map(|task| task.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            format!("{} [{}]", profile.name, tasks)
        })
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::{
        adapter::AdapterRegistry,
        config::load_workspace,
        core::{
            ExecutionMode, Module, ModuleId, PersistentReadiness, Profile, Task, TaskCommand,
            Workspace,
        },
        engine::planner::{plan_profile_task, plan_workspace},
        exec::render_resource_group,
    };

    fn module(name: &str) -> Module {
        module_with_manifest(name, "Cargo.toml")
    }

    fn module_with_manifest(name: &str, manifest: &str) -> Module {
        Module {
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
            cache_args: false,
            persistent: false,
            readiness: PersistentReadiness::Started,
            readiness_timeout: std::time::Duration::from_secs(30),
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
            .find(|unit| unit.scope.as_deref() == Some("contrib"))
            .expect("contrib scope unit exists");
        assert!(scoped.id.starts_with("rust/contrib/test/"));
        assert_eq!(scoped.modules[0].name.as_str(), "contrib-app");
        assert_eq!(scoped.argv_template[1], "check");
        assert_eq!(scoped.mode, ExecutionMode::SpawnEach);
        assert_eq!(
            render_resource_group(scoped, &plan.workspace.root).expect("resource group renders"),
            "scope:.:contrib-app"
        );

        let base_modules = plan
            .units
            .iter()
            .filter(|unit| unit.scope.is_none())
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
}
