//! Strict `toven.toml` document model.

use std::{collections::BTreeMap, path::PathBuf};

use crate::core::ExecutionMode;

/// Top-level `toven.toml` document.
#[derive(Debug, Clone, Eq, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigDocument {
    /// Workspace metadata.
    #[serde(default)]
    pub workspace: WorkspaceConfig,
    /// Named language profiles.
    #[serde(default)]
    pub profiles: BTreeMap<String, ProfileConfig>,
}

impl rskit_validation::Validate for ConfigDocument {
    fn validate(&self) -> Result<(), rskit_validation::validator::ValidationErrors> {
        Ok(())
    }
}

/// `[workspace]` table from `toven.toml`.
#[derive(Debug, Clone, Eq, PartialEq, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfig {
    /// Config schema version.
    pub schema: Option<u16>,
    /// Human-readable workspace name.
    pub name: Option<String>,
    /// Workspace root, relative to the config file unless absolute.
    pub root: Option<PathBuf>,
}

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

/// Task definition from `toven.toml`.
#[derive(Debug, Clone, Eq, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskConfig {
    /// Named preset to resolve for the owning profile language.
    pub preset: Option<String>,
    /// Direct argv template.
    pub argv: Option<Vec<String>>,
}
