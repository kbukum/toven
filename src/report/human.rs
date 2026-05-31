//! Human-readable plan report.

use std::fmt::Write as _;

use crate::{
    core::{
        AppResult, CommandOrigin, Module, Plan, TaskOrigin, scoped_module_display,
        scoped_module_key,
    },
    engine::graph::{dependency_key_for, selected_modules_by_name},
    exec::{render_execution_unit, render_resource_group},
};

/// Render a plan for terminal review.
pub fn render_human_plan(plan: &Plan) -> AppResult<String> {
    let mut output = String::new();
    writeln!(&mut output, "workspace: {}", plan.workspace.name).expect("write string");
    writeln!(&mut output, "root: {}", plan.workspace.root.display()).expect("write string");

    if plan.units.is_empty() {
        writeln!(&mut output, "\nno units").expect("write string");
        return Ok(output);
    }

    for unit in &plan.units {
        let argv = render_execution_unit(unit, &plan.workspace.root)?;
        let resource_group = render_resource_group(unit, &plan.workspace.root)?;
        writeln!(&mut output, "\nunit: {}", unit.id).expect("write string");
        writeln!(
            &mut output,
            "scope: {} adapter: {} task: {} mode: {}",
            unit.scope_id, unit.adapter_id, unit.task, unit.mode
        )
        .expect("write string");
        writeln!(
            &mut output,
            "command: {}",
            command_origin(&unit.command_origin)
        )
        .expect("write string");
        writeln!(
            &mut output,
            "task_origin: {}",
            task_origin(&unit.task_origin)
        )
        .expect("write string");
        writeln!(&mut output, "resource_group: {resource_group}").expect("write string");
        writeln!(&mut output, "argv: {argv:?}").expect("write string");
        let modules = unit
            .modules
            .iter()
            .map(|module| module.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(&mut output, "modules: {modules}").expect("write string");
        if let Some(dependencies) = module_dependencies(&unit.modules) {
            writeln!(&mut output, "dependencies: {dependencies}").expect("write string");
        }
    }

    Ok(output)
}

fn command_origin(origin: &CommandOrigin) -> String {
    match origin {
        CommandOrigin::DirectArgv => "direct argv".to_string(),
        CommandOrigin::Preset { name, language } => format!("preset {language}/{name}"),
    }
}

fn task_origin(origin: &TaskOrigin) -> String {
    match origin {
        TaskOrigin::AdapterDefault { adapter_id } => format!("adapter default {adapter_id}"),
        TaskOrigin::ProjectDefault => "project default".to_string(),
        TaskOrigin::ScopeOverride { scope_id } => format!("scope override {scope_id}"),
    }
}

fn module_dependencies(modules: &[Module]) -> Option<String> {
    let modules_by_key = modules
        .iter()
        .map(|module| (scoped_module_key(module), module))
        .collect::<std::collections::BTreeMap<_, _>>();
    let selected_by_name = selected_modules_by_name(modules_by_key.keys());
    let dependencies = modules
        .iter()
        .filter_map(|module| {
            let dependencies = module
                .dependencies
                .iter()
                .filter_map(|dependency| dependency_key_for(module, dependency, &selected_by_name))
                .map(|dependency| scoped_module_display(&dependency))
                .collect::<Vec<_>>()
                .join(", ");
            (!dependencies.is_empty())
                .then(|| format!("{}/{} -> {dependencies}", module.scope_id, module.name))
        })
        .collect::<Vec<_>>();

    (!dependencies.is_empty()).then(|| dependencies.join("; "))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::{
        core::{
            AdapterId, CommandOrigin, ExecutionMode, ExecutionUnit, Module, ModuleId, Plan,
            ScopeId, TaskOrigin, Workspace,
        },
        report::render_human_plan,
    };

    fn module(name: &str, dependencies: &[&str]) -> Module {
        Module {
            scope_id: ScopeId::new("rust").expect("scope id"),
            adapter_id: AdapterId::new("rust").expect("adapter id"),
            name: ModuleId::new(name).expect("module id"),
            package: Some(name.to_string()),
            root: PathBuf::from(name),
            manifest: Some(PathBuf::from("Cargo.toml")),
            dependencies: dependencies
                .iter()
                .map(|dependency| ModuleId::new(*dependency).expect("module id"))
                .collect(),
            source_patterns: Vec::new(),
        }
    }

    #[test]
    fn renders_command_origin_and_dependencies() {
        let plan = Plan {
            workspace: Workspace {
                schema: 1,
                name: "fixture".to_string(),
                root: PathBuf::from("/workspace"),
                base_ref: None,
                profiles: Vec::new(),
            },
            units: vec![ExecutionUnit {
                id: "rust/test/w0/batch".to_string(),
                scope_id: ScopeId::new("rust").expect("scope id"),
                adapter_id: AdapterId::new("rust").expect("adapter id"),
                task: "test".to_string(),
                command_origin: CommandOrigin::Preset {
                    name: "cargo-nextest".to_string(),
                    language: "rust".to_string(),
                },
                task_origin: TaskOrigin::ProjectDefault,
                mode: ExecutionMode::BatchReady,
                resource_group: "cargo:{project.root}".to_string(),
                modules: vec![module("core", &[]), module("api", &["core"])],
                argv_template: vec!["cargo".to_string(), "test".to_string()],
                module_arg_template: Vec::new(),
                passthrough_args: Vec::new(),
                cache_args: false,
                persistent: false,
                readiness: crate::core::PersistentReadiness::Started,
                readiness_timeout: std::time::Duration::from_secs(30),
                shared_inputs: Vec::new(),
            }],
        };

        let output = render_human_plan(&plan).expect("plan renders");

        assert!(output.contains("command: preset rust/cargo-nextest"));
        assert!(output.contains("task_origin: project default"));
        assert!(output.contains("dependencies: rust/api -> rust/core"));
    }
}
