//! Wave-ordering policy — an engine-owned named strategy.

use serde::{Deserialize, Serialize};

/// How active modules are ordered into ready-waves.
///
/// An **engine-owned named policy** selected by config, orthogonal to
/// [`FanOut`](crate::task::FanOut). The adapter supplies a per-kind default; the
/// user overrides at the ecosystem or task level via `run_strategy = "..."`.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunStrategy {
    /// Dependency-respecting waves; deps run before dependents.
    LeafToTop,
    /// Ignore the dep graph; collapse everything into a single wave.
    Unordered,
}
