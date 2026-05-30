//! Task-level config normalization.

use std::time::Duration;

use crate::{
    core::{
        AppError, AppResult, PersistentReadiness, Task, TaskCommand, validate_command_template,
        validate_identifier,
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
    /// Include passthrough args in cache keys instead of disabling cache.
    #[serde(default)]
    pub cache_args: bool,
    /// Keep the task process alive and wait for readiness.
    #[serde(default)]
    pub persistent: bool,
    /// Persistent task readiness shortcut.
    pub ready_on: Option<String>,
    /// Persistent task health command.
    pub ready_command: Option<Vec<String>>,
    /// Literal stdout/stderr text that marks a persistent task ready.
    pub ready_output: Option<String>,
    /// Persistent readiness timeout in seconds.
    pub ready_timeout_seconds: Option<u64>,
}

pub(super) fn normalize_task(
    profile_name: &str,
    language: &str,
    name: String,
    config: TaskConfig,
    resolver: &PresetResolver,
) -> AppResult<Task> {
    validate_identifier(format!("profiles.{profile_name}.tasks.{name}"), &name)?;
    let readiness = normalize_readiness(profile_name, &name, &config)?;
    let readiness_timeout = readiness_timeout(&config);

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

    Ok(Task {
        name,
        command,
        cache_args: config.cache_args,
        persistent: config.persistent,
        readiness,
        readiness_timeout,
    })
}

fn normalize_readiness(
    profile_name: &str,
    task_name: &str,
    config: &TaskConfig,
) -> AppResult<PersistentReadiness> {
    let configured = usize::from(config.ready_on.is_some())
        + usize::from(config.ready_command.is_some())
        + usize::from(config.ready_output.is_some());
    if configured > 1 {
        return Err(AppError::invalid_input(
            format!("profiles.{profile_name}.tasks.{task_name}"),
            "persistent readiness must define only one of ready_on, ready_command, or ready_output",
        ));
    }
    if !config.persistent && configured > 0 {
        return Err(AppError::invalid_input(
            format!("profiles.{profile_name}.tasks.{task_name}"),
            "readiness options require persistent = true",
        ));
    }
    if !config.persistent && config.ready_timeout_seconds.is_some() {
        return Err(AppError::invalid_input(
            format!("profiles.{profile_name}.tasks.{task_name}.ready_timeout_seconds"),
            "ready_timeout_seconds requires persistent = true",
        ));
    }
    if !config.persistent {
        return Ok(PersistentReadiness::Started);
    }

    if let Some(value) = &config.ready_on {
        if value != "started" {
            return Err(AppError::invalid_input(
                format!("profiles.{profile_name}.tasks.{task_name}.ready_on"),
                "ready_on must be 'started'",
            ));
        }
        return Ok(PersistentReadiness::Started);
    }
    if let Some(argv) = &config.ready_command {
        validate_command_template(
            format!("profiles.{profile_name}.tasks.{task_name}.ready_command"),
            argv,
        )?;
        return Ok(PersistentReadiness::Command(argv.clone()));
    }
    if let Some(output) = &config.ready_output {
        if output.is_empty() {
            return Err(AppError::invalid_input(
                format!("profiles.{profile_name}.tasks.{task_name}.ready_output"),
                "ready_output cannot be empty",
            ));
        }
        return Ok(PersistentReadiness::OutputContains(output.clone()));
    }
    Ok(PersistentReadiness::Started)
}

pub(super) fn readiness_timeout(config: &TaskConfig) -> Duration {
    Duration::from_secs(config.ready_timeout_seconds.unwrap_or(30))
}
