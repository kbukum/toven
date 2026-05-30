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
    let argv_field = argv_field(unit);
    let module_arg_template_field = module_arg_template_field(unit);
    let mut argv = Vec::new();
    for value in &unit.argv_template {
        let template = parse_template(&argv_field, value)?;
        if template.contains(Placeholder::Args) {
            ensure_exact_placeholder(&argv_field, value, &template, Placeholder::Args)?;
            argv.extend(unit.passthrough_args.clone());
            continue;
        }
        if template.contains(Placeholder::ModuleArgs) {
            ensure_exact_placeholder(&argv_field, value, &template, Placeholder::ModuleArgs)?;
            if unit.module_arg_template.is_empty() {
                return Err(AppError::invalid_input(
                    &module_arg_template_field,
                    "{module.args} requires module_arg_template",
                ));
            }
            argv.extend(render_module_args(
                unit,
                workspace_root,
                &module_arg_template_field,
            )?);
            continue;
        }
        reject_batch_scalar_module_placeholders(&argv_field, unit, value, &template)?;
        argv.push(template.render_scalar(
            workspace_root,
            scalar_module(
                &argv_field,
                unit,
                template_contains_module_scalar(&template),
            )?,
        )?);
    }
    Ok(argv)
}

/// Render an execution unit's resource group template.
pub fn render_resource_group(unit: &ExecutionUnit, workspace_root: &Path) -> AppResult<String> {
    let field = resource_group_field(unit);
    let template = parse_template(&field, &unit.resource_group)?;
    reject_batch_scalar_module_placeholders(&field, unit, &unit.resource_group, &template)?;
    template.render_scalar(
        workspace_root,
        scalar_module(&field, unit, template_contains_module_scalar(&template))?,
    )
}

fn render_module_args(
    unit: &ExecutionUnit,
    workspace_root: &Path,
    field: &str,
) -> AppResult<Vec<String>> {
    let mut rendered = Vec::new();
    for module in &unit.modules {
        for value in &unit.module_arg_template {
            let template = parse_template(field, value)?;
            if template.contains(Placeholder::Args) || template.contains(Placeholder::ModuleArgs) {
                return Err(AppError::invalid_input(
                    field,
                    "module_arg_template cannot contain selector placeholders",
                ));
            }
            rendered.push(template.render_scalar(workspace_root, Some(module))?);
        }
    }
    Ok(rendered)
}

fn scalar_module<'a>(
    field: &str,
    unit: &'a ExecutionUnit,
    required: bool,
) -> AppResult<Option<&'a Module>> {
    if !required {
        return Ok(None);
    }
    if unit.modules.len() == 1 {
        return Ok(unit.modules.first());
    }
    Err(AppError::invalid_input(
        field,
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
                unit.mode
            ),
        ));
    }
    Ok(())
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

fn parse_template(field: &str, value: &str) -> AppResult<Template> {
    Template::parse(value).map_err(|error| AppError::invalid_input(field, error.message))
}

fn argv_field(unit: &ExecutionUnit) -> String {
    format!("profiles.{}.tasks.{}.argv", unit.profile, unit.task)
}

fn module_arg_template_field(unit: &ExecutionUnit) -> String {
    format!("profiles.{}.module_arg_template", unit.profile)
}

fn resource_group_field(unit: &ExecutionUnit) -> String {
    format!("profiles.{}.resource_group", unit.profile)
}

#[cfg(test)]
mod tests {
    use crate::{
        core::{CommandOrigin, ExecutionMode, ExecutionUnit, Module, ModuleId},
        exec::{render_execution_unit, render_resource_group},
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
            command_origin: CommandOrigin::DirectArgv,
            mode: ExecutionMode::BatchReady,
            resource_group: "{workspace.root}".to_string(),
            modules,
            argv_template,
            module_arg_template: vec!["-p".to_string(), "{module.package}".to_string()],
            passthrough_args: vec!["--release".to_string()],
            shared_inputs: Vec::new(),
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

        assert!(error.message.contains("profiles.rust.tasks.test.argv"));
        assert!(error.message.contains("complete argv item"));
    }

    #[test]
    fn rejects_scalar_module_placeholder_in_batch_mode() {
        let error = render_execution_unit(
            &unit(vec!["{module.package}".to_string()], vec![module("core")]),
            std::path::Path::new("/workspace"),
        )
        .expect_err("batch scalar module placeholder should fail");

        assert!(error.message.contains("profiles.rust.tasks.test.argv"));
        assert!(error.message.contains("scalar module"));
    }

    #[test]
    fn reports_module_arg_template_field_for_missing_template() {
        let mut unit = unit(
            vec!["cargo".to_string(), "{module.args}".to_string()],
            vec![module("core")],
        );
        unit.module_arg_template = Vec::new();

        let error = render_execution_unit(&unit, std::path::Path::new("/workspace"))
            .expect_err("module args without template should fail");

        assert!(error.message.contains("profiles.rust.module_arg_template"));
    }

    #[test]
    fn reports_resource_group_field_for_scalar_module_errors() {
        let mut unit = unit(
            vec!["cargo".to_string()],
            vec![module("core"), module("app")],
        );
        unit.mode = ExecutionMode::SpawnEach;
        unit.resource_group = "cargo:{module.package}".to_string();

        let error = render_resource_group(&unit, std::path::Path::new("/workspace"))
            .expect_err("resource group with many modules should fail");

        assert!(error.message.contains("profiles.rust.resource_group"));
        assert!(!error.message.contains("invalid argv"));
    }
}
