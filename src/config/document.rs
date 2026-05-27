//! Strict `toven.toml` document model.

use std::collections::BTreeMap;

use crate::config::{ProfileConfig, WorkspaceConfig};

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
