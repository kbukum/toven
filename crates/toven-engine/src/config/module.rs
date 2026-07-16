//! `[modules.<ecosystem:module>]` — the per-module release override section.

use serde::{Deserialize, Serialize};
use toven_ports::ReleaseConfig;

/// Per-module configuration, keyed by an `ecosystem:module` reference.
///
/// Today this carries only the release override: a `[modules.<name>.release]`
/// block whose set fields win over the module's `[ecosystems.<id>].release`
/// default (see [`merge_release`](toven_ports::merge_release)). The section is
/// strict — an unknown key is a typed load error — and every field defaults, so
/// an absent `[modules]` table changes nothing.
#[derive(Debug, Clone, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleConfig {
    /// Per-module release override folded onto the ecosystem release default.
    #[serde(default, skip_serializing_if = "ReleaseConfig::is_default")]
    pub release: ReleaseConfig,
}
