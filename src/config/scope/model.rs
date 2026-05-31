//! Optional scope override config normalization.

use std::collections::BTreeMap;

use crate::{
    config::{TaskConfig, task::normalize_task},
    core::{
        AdapterOptions, AppResult, ExecutionMode, ScopeOverride, validate_identifier,
        validate_template, validate_templates,
    },
    preset::PresetResolver,
};

/// `[scopes.<name>]` override table from `toven.toml`.
#[derive(Debug, Clone, Eq, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeConfig {
    /// Profile this scope overrides.
    pub profile: String,
    /// Adapter-owned discovery override/filter options.
    #[serde(default)]
    pub discovery: AdapterOptions,
    /// Execution mode override.
    pub execution: Option<ExecutionMode>,
    /// Module argument template override.
    pub module_arg_template: Option<Vec<String>>,
    /// Resource group template override.
    pub resource_group: Option<String>,
    /// Task overrides/replacements.
    #[serde(default)]
    pub tasks: BTreeMap<String, TaskConfig>,
}

pub(crate) fn normalize_scope_overrides(
    scopes: BTreeMap<String, ScopeConfig>,
    resolver: &PresetResolver,
) -> AppResult<Vec<(String, ScopeOverride, String)>> {
    let mut normalized = Vec::with_capacity(scopes.len());
    for (name, scope) in scopes {
        normalized.push(normalize_scope_override(name, scope, resolver)?);
    }
    Ok(normalized)
}

fn normalize_scope_override(
    name: String,
    config: ScopeConfig,
    resolver: &PresetResolver,
) -> AppResult<(String, ScopeOverride, String)> {
    validate_identifier(format!("scopes.{name}"), &name)?;
    validate_identifier(format!("scopes.{name}.profile"), &config.profile)?;

    if let Some(module_arg_template) = &config.module_arg_template {
        validate_templates(
            format!("scopes.{name}.module_arg_template"),
            module_arg_template,
        )?;
    }
    if let Some(resource_group) = &config.resource_group {
        validate_template(format!("scopes.{name}.resource_group"), resource_group)?;
    }

    let mut tasks = Vec::with_capacity(config.tasks.len());
    for (task_name, task) in config.tasks {
        tasks.push(normalize_task(
            &name,
            &config.profile,
            task_name,
            task,
            resolver,
            "scopes",
        )?);
    }

    Ok((
        name.clone(),
        ScopeOverride {
            name,
            adapter_options: config.discovery,
            execution: config.execution,
            module_arg_template: config.module_arg_template,
            resource_group: config.resource_group,
            tasks,
        },
        config.profile,
    ))
}
