//! Execution unit planning.

use std::collections::BTreeMap;

use crate::{
    core::{
        AppError, AppResult, DISCOVERY_SCHEMA_VERSION, DiscoverRequest, ExecutionMode,
        ExecutionUnit, Plan, Profile, Task, TaskCommand, Workspace,
    },
    engine::scheduler::ready_waves,
    exec::render_execution_unit,
    lang::LangRegistry,
};

/// Build a task plan for every profile that defines `task_name`.
pub fn plan_workspace(
    workspace: Workspace,
    task_name: &str,
    passthrough_args: &[String],
    registry: &LangRegistry,
) -> AppResult<Plan> {
    let mut units = Vec::new();
    let mut matched = false;
    let mut discovery_cache = BTreeMap::new();

    for profile in &workspace.profiles {
        let Some(task) = profile.tasks.iter().find(|task| task.name == task_name) else {
            continue;
        };
        matched = true;
        let modules =
            discover_profile_modules(&workspace, profile, registry, &mut discovery_cache)?;
        units.extend(plan_profile_task(
            &workspace,
            profile,
            task,
            modules,
            passthrough_args,
        )?);
    }

    if !matched {
        return Err(AppError::invalid_input(
            "task",
            format!(
                "task '{task_name}' is not defined by any profile; available tasks: {}",
                available_tasks(&workspace)
            ),
        ));
    }

    Ok(Plan { workspace, units })
}

fn discover_profile_modules(
    workspace: &Workspace,
    profile: &Profile,
    registry: &LangRegistry,
    cache: &mut BTreeMap<String, Vec<crate::core::Module>>,
) -> AppResult<Vec<crate::core::Module>> {
    let cache_key = format!("{}:{:?}", profile.language, profile.discovery_command);
    if let Some(modules) = cache.get(&cache_key) {
        return Ok(modules.clone());
    }

    let adapter = registry.adapter_for_profile(profile)?;
    let response = adapter.discover(&DiscoverRequest {
        schema_version: DISCOVERY_SCHEMA_VERSION,
        workspace_root: workspace.root.clone(),
    })?;
    if response.schema_version != DISCOVERY_SCHEMA_VERSION {
        return Err(AppError::invalid_input(
            format!("profiles.{}.language", profile.name),
            format!(
                "unsupported discovery response schema {}",
                response.schema_version
            ),
        ));
    }
    cache.insert(cache_key, response.modules.clone());
    Ok(response.modules)
}

fn plan_profile_task(
    workspace: &Workspace,
    profile: &Profile,
    task: &Task,
    modules: Vec<crate::core::Module>,
    passthrough_args: &[String],
) -> AppResult<Vec<ExecutionUnit>> {
    let argv_template = task_argv(task)?;
    let waves = ready_waves(&modules)?;
    let mut units = Vec::new();

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
                        argv_template.clone(),
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
                    argv_template.clone(),
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
                argv_template,
                passthrough_args.to_owned(),
            ));
        }
    }

    for unit in &units {
        render_execution_unit(unit, &workspace.root)?;
    }
    Ok(units)
}

fn unit(
    profile: &Profile,
    task: &Task,
    id: String,
    modules: Vec<crate::core::Module>,
    argv_template: Vec<String>,
    passthrough_args: Vec<String>,
) -> ExecutionUnit {
    ExecutionUnit {
        id,
        profile: profile.name.clone(),
        task: task.name.clone(),
        mode: profile.execution,
        resource_group: profile.resource_group.clone(),
        modules,
        argv_template,
        module_arg_template: profile.module_arg_template.clone(),
        passthrough_args,
    }
}

fn task_argv(task: &Task) -> AppResult<Vec<String>> {
    match &task.command {
        TaskCommand::Argv(argv) => Ok(argv.clone()),
        TaskCommand::ResolvedPreset(preset) => Ok(preset.argv.clone()),
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
