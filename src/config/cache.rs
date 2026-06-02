//! Cache policy config normalization.

use crate::core::{CacheLocation, CacheSettings};

/// `[cache]` table from `toven.toml`.
#[derive(Debug, Clone, Copy, Eq, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheConfig {
    /// Cache storage location.
    #[serde(default = "default_location")]
    pub location: CacheLocationConfig,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            location: CacheLocationConfig::User,
        }
    }
}

/// Configured cache location.
#[derive(Debug, Clone, Copy, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CacheLocationConfig {
    /// Use the platform user cache directory.
    User,
    /// Use the workspace-local `.toven/cache` directory.
    Workspace,
}

pub(super) const fn normalize_cache_config(config: CacheConfig) -> CacheSettings {
    CacheSettings {
        location: match config.location {
            CacheLocationConfig::User => CacheLocation::User,
            CacheLocationConfig::Workspace => CacheLocation::Workspace,
        },
    }
}

const fn default_location() -> CacheLocationConfig {
    CacheLocationConfig::User
}
