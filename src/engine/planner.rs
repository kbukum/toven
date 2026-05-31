//! Execution unit planning.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    adapter::AdapterRegistry,
    core::{
        AdapterId, AdapterOptions, AppError, AppResult, CommandOrigin, DISCOVERY_SCHEMA_VERSION,
        DiscoverRequest, ExecutionMode, ExecutionUnit, ModuleId, Plan, Profile, ScopeId, Task,
        TaskCommand, Workspace, validate_discovery_response,
    },
    engine::scheduler::ready_waves,
    exec::{render_execution_unit, render_resource_group},
};

/// Modules discovered for one profile/task pair.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DiscoveredTaskProfile {
    /// Profile that owns the task.
    pub profile: Profile,
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
        let Some(task) = profile.tasks.iter().find(|task| task.name == task_name) else {
            continue;
        };
        let modules = discover_profile_modules(workspace, profile, registry, &mut discovery_cache)?;
        discovered.push(DiscoveredTaskProfile {
            profile: profile.clone(),
            task: task.clone(),
            modules,
        });
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

    for discovered in discovered {
        let modules = filter_modules(discovered.modules.clone(), module_filter);
        units.extend(plan_profile_task(
            &workspace,
            &discovered.profile,
            &discovered.task,
            modules,
            passthrough_args,
        )?);
    }

    Ok(Plan { workspace, units })
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
    let scope_id = ScopeId::new(profile.name.clone())?;
    let adapter_id = AdapterId::new(profile.language.clone())?;
    let cache_key = format!("{scope_id}:{adapter_id}:{:?}", profile.discovery_command);
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
        adapter_options: AdapterOptions::default(),
    };
    let response = adapter.discover(&request)?;
    validate_discovery_response(
        format!("profiles.{}.language", profile.name),
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

fn plan_profile_task(
    workspace: &Workspace,
    profile: &Profile,
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
        ExecutionMode::SpawnEach => {
            for (wave_index, wave) in waves.into_iter().enumerate() {
                for module in wave {
                    units.push(unit(
                        profile,
                        task,
                        format!(
                            "{}/{}/w{wave_index}/{}",
                            profile.name, task.name, module.name
                        ),
                        vec![module],
                        command.clone(),
                        passthrough_args.to_owned(),
                    ));
                }
            }
        }
        ExecutionMode::BatchReady => {
            for (wave_index, wave) in waves.into_iter().enumerate() {
                units.push(unit(
                    profile,
                    task,
                    format!("{}/{}/w{wave_index}/batch", profile.name, task.name),
                    wave,
                    command.clone(),
                    passthrough_args.to_owned(),
                ));
            }
        }
        ExecutionMode::WorkspaceOnce => {
            units.push(unit(
                profile,
                task,
                format!("{}/{}/workspace", profile.name, task.name),
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

fn unit(
    profile: &Profile,
    task: &Task,
    id: String,
    modules: Vec<crate::core::Module>,
    command: PlannedCommand,
    passthrough_args: Vec<String>,
) -> ExecutionUnit {
    ExecutionUnit {
        id,
        profile: profile.name.clone(),
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
        core::{
            ExecutionMode, Module, ModuleId, PersistentReadiness, Profile, Task, TaskCommand,
            Workspace,
        },
        engine::planner::plan_profile_task,
    };

    fn module(name: &str) -> Module {
        Module {
            name: ModuleId::new(name).expect("module id"),
            package: Some(format!("{name}-pkg")),
            root: PathBuf::from(name),
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
            discovery_command: None,
            execution: ExecutionMode::BatchReady,
            module_arg_template: Vec::new(),
            resource_group: "cargo:{module.package}".to_string(),
            tasks: vec![task()],
        };

        let error = plan_profile_task(
            &workspace,
            &profile,
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
            discovery_command: None,
            execution: ExecutionMode::WorkspaceOnce,
            module_arg_template: Vec::new(),
            resource_group: "cargo:{workspace.root}".to_string(),
            tasks: vec![task()],
        };

        let units = plan_profile_task(&workspace, &profile, &profile.tasks[0], Vec::new(), &[])
            .expect("empty planning succeeds");

        assert!(units.is_empty());
    }
}
