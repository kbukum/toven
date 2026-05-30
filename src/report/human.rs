//! Human-readable plan report.

use std::fmt::Write as _;

use crate::{
    core::{AppResult, CommandOrigin, Module, Plan},
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
            "profile: {} task: {} mode: {}",
            unit.profile, unit.task, unit.mode
        )
        .expect("write string");
        writeln!(
            &mut output,
            "command: {}",
            command_origin(&unit.command_origin)
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

fn module_dependencies(modules: &[Module]) -> Option<String> {
    let dependencies = modules
        .iter()
        .filter(|module| !module.dependencies.is_empty())
        .map(|module| {
            let dependencies = module
                .dependencies
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            format!("{} -> {dependencies}", module.name)
        })
        .collect::<Vec<_>>();

    (!dependencies.is_empty()).then(|| dependencies.join("; "))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::{
        core::{CommandOrigin, ExecutionMode, ExecutionUnit, Module, ModuleId, Plan, Workspace},
        report::render_human_plan,
    };

    fn module(name: &str, dependencies: &[&str]) -> Module {
        Module {
            name: ModuleId::new(name).expect("module id"),
            package: Some(name.to_string()),
            root: PathBuf::from(name),
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
                profile: "rust".to_string(),
                task: "test".to_string(),
                command_origin: CommandOrigin::Preset {
                    name: "cargo-nextest".to_string(),
                    language: "rust".to_string(),
                },
                mode: ExecutionMode::BatchReady,
                resource_group: "cargo:{workspace.root}".to_string(),
                modules: vec![module("core", &[]), module("api", &["core"])],
                argv_template: vec!["cargo".to_string(), "test".to_string()],
                module_arg_template: Vec::new(),
                passthrough_args: Vec::new(),
            }],
        };

        let output = render_human_plan(&plan).expect("plan renders");

        assert!(output.contains("command: preset rust/cargo-nextest"));
        assert!(output.contains("dependencies: api -> core"));
    }
}
