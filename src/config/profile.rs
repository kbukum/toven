//! Profile-level config normalization.

use std::collections::BTreeMap;

use crate::{
    config::{TaskConfig, task::normalize_task},
    core::{AppError, AppResult, ExecutionMode, Profile},
    preset::PresetResolver,
    validation::{
        validate_command_template, validate_identifier, validate_template, validate_templates,
    },
};

const DEFAULT_RESOURCE_GROUP: &str = "{workspace.root}";

/// `[profiles.<name>]` table from `toven.toml`.
#[derive(Debug, Clone, Eq, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileConfig {
    /// Language identifier.
    pub language: String,
    /// Optional command adapter override.
    pub discovery_command: Option<Vec<String>>,
    /// Execution mode for tasks in this profile.
    pub execution: Option<ExecutionMode>,
    /// Template for rendering one module selector.
    pub module_arg_template: Option<Vec<String>>,
    /// Resource group template used for scheduling/reporting.
    pub resource_group: Option<String>,
    /// Configured tasks.
    #[serde(default)]
    pub tasks: BTreeMap<String, TaskConfig>,
}

pub(super) fn normalize_profiles(
    profiles: BTreeMap<String, ProfileConfig>,
    resolver: &PresetResolver,
) -> AppResult<Vec<Profile>> {
    if profiles.is_empty() {
        return Err(AppError::invalid_input(
            "profiles",
            "at least one profile is required",
        ));
    }

    let mut normalized = Vec::with_capacity(profiles.len());
    for (name, profile) in profiles {
        normalized.push(normalize_profile(name, profile, resolver)?);
    }
    Ok(normalized)
}

fn normalize_profile(
    name: String,
    config: ProfileConfig,
    resolver: &PresetResolver,
) -> AppResult<Profile> {
    validate_identifier("profiles", &name)?;
    validate_identifier("profiles.language", &config.language)?;
    validate_discovery_command(&name, config.discovery_command.as_deref())?;

    if config.tasks.is_empty() {
        return Err(AppError::invalid_input(
            format!("profiles.{name}.tasks"),
            "at least one task is required",
        ));
    }

    let module_arg_template = config.module_arg_template.unwrap_or_default();
    validate_templates(
        format!("profiles.{name}.module_arg_template"),
        &module_arg_template,
    )?;

    let resource_group = config
        .resource_group
        .unwrap_or_else(|| DEFAULT_RESOURCE_GROUP.to_string());
    validate_template(format!("profiles.{name}.resource_group"), &resource_group)?;

    let mut tasks = Vec::with_capacity(config.tasks.len());
    for (task_name, task) in config.tasks {
        tasks.push(normalize_task(
            &name,
            &config.language,
            task_name,
            task,
            resolver,
        )?);
    }

    Ok(Profile {
        name,
        language: config.language,
        discovery_command: config.discovery_command,
        execution: config.execution.unwrap_or(ExecutionMode::SpawnEach),
        module_arg_template,
        resource_group,
        tasks,
    })
}

fn validate_discovery_command(profile_name: &str, command: Option<&[String]>) -> AppResult<()> {
    if let Some(command) = command {
        validate_command_template(
            format!("profiles.{profile_name}.discovery_command"),
            command,
        )?;
    }
    Ok(())
}
