//! Profile-level config normalization.

use std::{collections::BTreeMap, path::Path};

use crate::{
    config::{TaskConfig, task::normalize_task},
    core::{
        AdapterOptions, AppError, AppResult, ExecutionMode, Profile, ScopeOverride,
        validate_command_template, validate_identifier, validate_template, validate_templates,
    },
    preset::PresetResolver,
};

const DEFAULT_RESOURCE_GROUP: &str = "{project.root}";

/// `[profiles.<name>]` table from `toven.toml`.
#[derive(Debug, Clone, Eq, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileConfig {
    /// Adapter identifier.
    pub adapter: String,
    /// Adapter-owned discovery options.
    #[serde(default)]
    pub discovery: AdapterOptions,
    /// Optional command discovery override.
    pub discover: Option<Vec<String>>,
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
    project_root: &Path,
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
        normalized.push(normalize_profile(name, profile, project_root, resolver)?);
    }
    Ok(normalized)
}

fn normalize_profile(
    name: String,
    config: ProfileConfig,
    _project_root: &Path,
    resolver: &PresetResolver,
) -> AppResult<Profile> {
    validate_identifier(format!("profiles.{name}"), &name)?;
    validate_identifier(format!("profiles.{name}.adapter"), &config.adapter)?;
    validate_discover(&name, config.discover.as_deref())?;

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
            &config.adapter,
            task_name,
            task,
            resolver,
            "profiles",
        )?);
    }

    Ok(Profile {
        name,
        language: config.adapter,
        adapter_options: config.discovery,
        discovery_command: config.discover,
        execution: config.execution.unwrap_or(ExecutionMode::SpawnEach),
        module_arg_template,
        resource_group,
        tasks,
        scope_overrides: Vec::new(),
    })
}

fn validate_discover(profile_name: &str, command: Option<&[String]>) -> AppResult<()> {
    if let Some(command) = command {
        validate_command_template(format!("profiles.{profile_name}.discover"), command)?;
    }
    Ok(())
}

pub(super) fn attach_scope_overrides(
    profiles: &mut [Profile],
    overrides: Vec<(String, ScopeOverride, String)>,
) -> AppResult<()> {
    for (scope_name, override_config, profile_name) in overrides {
        let profile = profiles
            .iter_mut()
            .find(|profile| profile.name == profile_name)
            .ok_or_else(|| {
                AppError::invalid_input(
                    format!("scopes.{scope_name}.profile"),
                    format!("unknown profile '{profile_name}'"),
                )
            })?;
        profile.scope_overrides.push(override_config);
    }
    Ok(())
}
