//! Strict `toven.toml` data model.

use std::{collections::BTreeMap, path::PathBuf};

use crate::core::ExecutionMode;

/// Raw top-level `toven.toml` document.
#[derive(Debug, Clone, Eq, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawConfig {
    /// Workspace metadata.
    #[serde(default)]
    pub workspace: RawWorkspace,
    /// Named language profiles.
    #[serde(default)]
    pub profiles: BTreeMap<String, RawProfile>,
}

impl rskit_validation::Validate for RawConfig {
    fn validate(&self) -> Result<(), rskit_validation::validator::ValidationErrors> {
        Ok(())
    }
}

/// Raw `[workspace]` table.
#[derive(Debug, Clone, Eq, PartialEq, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawWorkspace {
    /// Config schema version.
    pub schema: Option<u16>,
    /// Human-readable workspace name.
    pub name: Option<String>,
    /// Workspace root, relative to the config file unless absolute.
    pub root: Option<PathBuf>,
}

/// Raw `[profiles.<name>]` table.
#[derive(Debug, Clone, Eq, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawProfile {
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
    pub tasks: BTreeMap<String, RawTask>,
}

/// Raw task definition.
#[derive(Debug, Clone, Eq, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawTask {
    /// Named preset to resolve for the owning profile language.
    pub preset: Option<String>,
    /// Direct argv template.
    pub argv: Option<Vec<String>>,
}
