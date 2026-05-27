//! Task-level config normalization.

use crate::{
    core::{
        AppError, AppResult, Task, TaskCommand, validate_command_template, validate_identifier,
    },
    preset::PresetResolver,
};

/// Task definition from `toven.toml`.
#[derive(Debug, Clone, Eq, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskConfig {
    /// Named preset to resolve for the owning profile language.
    pub preset: Option<String>,
    /// Direct argv template.
    pub argv: Option<Vec<String>>,
}

pub(super) fn normalize_task(
    profile_name: &str,
    language: &str,
    name: String,
    config: TaskConfig,
    resolver: &PresetResolver,
) -> AppResult<Task> {
    validate_identifier(format!("profiles.{profile_name}.tasks.{name}"), &name)?;

    let command = match (config.argv, config.preset) {
        (Some(argv), None) => {
            validate_command_template(format!("profiles.{profile_name}.tasks.{name}.argv"), &argv)?;
            TaskCommand::Argv(argv)
        }
        (None, Some(preset)) => {
            let preset_field = format!("profiles.{profile_name}.tasks.{name}.preset");
            validate_identifier(&preset_field, &preset)?;
            TaskCommand::ResolvedPreset(resolver.resolve_for_field(
                &preset_field,
                language,
                &preset,
            )?)
        }
        (Some(_), Some(_)) => {
            return Err(AppError::invalid_input(
                format!("profiles.{profile_name}.tasks.{name}"),
                "task must define either 'argv' or 'preset', not both",
            ));
        }
        (None, None) => {
            return Err(AppError::invalid_input(
                format!("profiles.{profile_name}.tasks.{name}"),
                "task must define either 'argv' or 'preset'",
            ));
        }
    };

    Ok(Task { name, command })
}
