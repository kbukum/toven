//! Release sub-config — the engine-common `release.*` knobs.

use serde::{Deserialize, Serialize};

/// The `[ecosystems.<id>] release.*` sub-config.
///
/// Both fields name **engine-owned** concepts (the bump policy and the registry)
/// resolved by the engine; the adapter only carries the user's selection through
/// to release-target wiring. The exact value sets land with the field-level
/// config schema.
#[derive(Debug, Clone, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseConfig {
    /// Named bump policy (e.g. `"semver-cascade"`); `None` = adapter default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy: Option<String>,
    /// Target registry identifier (e.g. `"crates-io"`); `None` = not publishable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
}
