//! Render execution units into argv.

use std::path::Path;

use crate::core::{
    AppError, AppResult, ExecutionMode, ExecutionUnit, Module, Placeholder, Template, TemplatePart,
};

/// Render an execution unit's argv template.
pub fn render_execution_unit(
    unit: &ExecutionUnit,
    workspace_root: &Path,
) -> AppResult<Vec<String>> {
    let mut argv = Vec::new();
    for value in &unit.argv_template {
        let template = Template::parse(value)?;
        if template.contains(Placeholder::Args) {
            ensure_exact_placeholder("argv", value, &template, Placeholder::Args)?;
            argv.extend(unit.passthrough_args.clone());
            continue;
        }
        if template.contains(Placeholder::ModuleArgs) {
            ensure_exact_placeholder("argv", value, &template, Placeholder::ModuleArgs)?;
            if unit.module_arg_template.is_empty() {
                return Err(AppError::invalid_input(
                    "module_arg_template",
                    "{module.args} requires module_arg_template",
                ));
            }
            argv.extend(render_module_args(unit, workspace_root)?);
            continue;
        }
        reject_batch_scalar_module_placeholders("argv", unit, value, &template)?;
        argv.push(template.render_scalar(
            workspace_root,
            scalar_module(unit, template_contains_module_scalar(&template))?,
        )?);
    }
    Ok(argv)
}

/// Render an execution unit's resource group template.
pub fn render_resource_group(unit: &ExecutionUnit, workspace_root: &Path) -> AppResult<String> {
    let template = Template::parse(&unit.resource_group)?;
    reject_batch_scalar_module_placeholders(
        "resource_group",
        unit,
        &unit.resource_group,
        &template,
    )?;
    template.render_scalar(
        workspace_root,
        scalar_module(unit, template_contains_module_scalar(&template))?,
    )
}

fn render_module_args(unit: &ExecutionUnit, workspace_root: &Path) -> AppResult<Vec<String>> {
    let mut rendered = Vec::new();
    for module in &unit.modules {
        for value in &unit.module_arg_template {
            let template = Template::parse(value)?;
            if template.contains(Placeholder::Args) || template.contains(Placeholder::ModuleArgs) {
                return Err(AppError::invalid_input(
                    "module_arg_template",
                    "module_arg_template cannot contain selector placeholders",
                ));
            }
            rendered.push(template.render_scalar(workspace_root, Some(module))?);
        }
    }
    Ok(rendered)
}

fn scalar_module(unit: &ExecutionUnit, required: bool) -> AppResult<Option<&Module>> {
    if !required {
        return Ok(None);
    }
    if unit.modules.len() == 1 {
        return Ok(unit.modules.first());
    }
    Err(AppError::invalid_input(
        "argv",
        "scalar module placeholders require exactly one module",
    ))
}

fn reject_batch_scalar_module_placeholders(
    field: &str,
    unit: &ExecutionUnit,
    value: &str,
    template: &Template,
) -> AppResult<()> {
    if unit.mode != ExecutionMode::SpawnEach && template_contains_module_scalar(template) {
        return Err(AppError::invalid_input(
            field,
            format!(
                "template '{value}' cannot use scalar module placeholders with {}",
                execution_mode_name(unit.mode)
            ),
        ));
    }
    Ok(())
}

const fn execution_mode_name(mode: ExecutionMode) -> &'static str {
    match mode {
        ExecutionMode::SpawnEach => "spawn-each",
        ExecutionMode::BatchReady => "batch-ready",
        ExecutionMode::WorkspaceOnce => "workspace-once",
    }
}

fn ensure_exact_placeholder(
    field: &str,
    value: &str,
    template: &Template,
    placeholder: Placeholder,
) -> AppResult<()> {
    if template.parts() == [TemplatePart::Placeholder(placeholder)] {
        return Ok(());
    }
    Err(AppError::invalid_input(
        field,
        format!("placeholder '{{{placeholder}}}' must be a complete argv item in '{value}'"),
    ))
}

fn template_contains_module_scalar(template: &Template) -> bool {
    template.contains(Placeholder::ModuleName)
        || template.contains(Placeholder::ModulePackage)
        || template.contains(Placeholder::ModulePath)
}

#[cfg(test)]
mod tests {
    use crate::{
        core::{ExecutionMode, ExecutionUnit, Module, ModuleId},
        exec::render_execution_unit,
    };

    fn module(name: &str) -> Module {
        Module {
            name: ModuleId::new(name).expect("module id"),
            package: Some(format!("{name}-pkg")),
            root: format!("crates/{name}").into(),
            dependencies: Vec::new(),
            source_patterns: Vec::new(),
        }
    }

    fn unit(argv_template: Vec<String>, modules: Vec<Module>) -> ExecutionUnit {
        ExecutionUnit {
            id: "unit".to_string(),
            profile: "rust".to_string(),
            task: "test".to_string(),
            mode: ExecutionMode::BatchReady,
            resource_group: "{workspace.root}".to_string(),
            modules,
            argv_template,
            module_arg_template: vec!["-p".to_string(), "{module.package}".to_string()],
            passthrough_args: vec!["--release".to_string()],
        }
    }

    #[test]
    fn expands_selector_placeholders_as_tokens() {
        let argv = render_execution_unit(
            &unit(
                vec![
                    "cargo".to_string(),
                    "test".to_string(),
                    "{module.args}".to_string(),
                    "{args}".to_string(),
                ],
                vec![module("core"), module("app")],
            ),
            std::path::Path::new("/workspace"),
        )
        .expect("argv renders");

        assert_eq!(
            argv,
            [
                "cargo",
                "test",
                "-p",
                "core-pkg",
                "-p",
                "app-pkg",
                "--release"
            ]
        );
    }

    #[test]
    fn rejects_embedded_passthrough_args() {
        let error = render_execution_unit(
            &unit(vec!["--features={args}".to_string()], vec![module("core")]),
            std::path::Path::new("/workspace"),
        )
        .expect_err("embedded args should fail");

        assert!(error.message.contains("complete argv item"));
    }

    #[test]
    fn rejects_scalar_module_placeholder_in_batch_mode() {
        let error = render_execution_unit(
            &unit(vec!["{module.package}".to_string()], vec![module("core")]),
            std::path::Path::new("/workspace"),
        )
        .expect_err("batch scalar module placeholder should fail");

        assert!(error.message.contains("scalar module"));
    }
}
