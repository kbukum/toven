//! `[modules.<ecosystem:module>]` — the per-module release override section.

use serde::{Deserialize, Serialize};
use toven_ports::{CoverageConfig, ReleaseConfig};

/// Per-module configuration, keyed by an `ecosystem:module` reference.
///
/// Carries the per-module release and coverage overrides: a
/// `[modules.<name>.release]` block whose set fields win over the module's
/// `[ecosystems.<id>].release` default (see
/// [`merge_release`](toven_ports::merge_release)), and a
/// `[modules.<name>.coverage]` block folded the same way over the ecosystem
/// coverage default (see [`merge_coverage`](toven_ports::merge_coverage)). The
/// section is strict — an unknown key is a typed load error — and every field
/// defaults, so an absent `[modules]` table changes nothing.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleConfig {
    /// Per-module release override folded onto the ecosystem release default.
    #[serde(default, skip_serializing_if = "ReleaseConfig::is_default")]
    pub release: ReleaseConfig,
    /// Per-module coverage override folded onto the ecosystem coverage default.
    #[serde(default, skip_serializing_if = "CoverageConfig::is_default")]
    pub coverage: CoverageConfig,
}
