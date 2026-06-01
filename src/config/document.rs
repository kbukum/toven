//! Strict `toven.toml` document model.

use std::collections::BTreeMap;

use crate::config::{
    CacheConfig, DependencyOverlayConfig, ProfileConfig, ProjectConfig, ScopeConfig,
};

/// Top-level `toven.toml` document.
#[derive(Debug, Clone, Eq, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigDocument {
    /// Project metadata.
    #[serde(default)]
    pub project: ProjectConfig,
    /// Named adapter profiles.
    #[serde(default)]
    pub profiles: BTreeMap<String, ProfileConfig>,
    /// Optional named scope overrides.
    #[serde(default)]
    pub scopes: BTreeMap<String, ScopeConfig>,
    /// Cache policy.
    #[serde(default)]
    pub cache: CacheConfig,
    /// Explicit cross-scope dependency overlays.
    #[serde(default)]
    pub overlays: Vec<DependencyOverlayConfig>,
}

impl rskit_validation::Validate for ConfigDocument {
    fn validate(&self) -> Result<(), rskit_validation::validator::ValidationErrors> {
        Ok(())
    }
}
