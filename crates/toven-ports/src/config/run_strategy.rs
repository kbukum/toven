//! Wave-ordering policy — an engine-owned named strategy.

use serde::{Deserialize, Serialize};

/// How active modules are ordered into ready-waves.
///
/// An **engine-owned named policy** selected by config, orthogonal to
/// [`FanOut`](crate::task::FanOut). The adapter supplies a per-kind default;
/// the user overrides at the ecosystem or task level via `run_strategy =
/// "..."`.
///
/// The two variants are the deliberate, complete set: dependency-respecting
/// waves (the safe default) and a single collapsed wave (opt-in for tasks with
/// no inter-module ordering constraint). A grouped/aggregate strategy is added
/// only when a real ecosystem need is demonstrated with tests and docs — no
/// speculative additions.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunStrategy {
    /// Dependency-respecting waves; deps run before dependents.
    LeafToTop,
    /// Ignore the dep graph; collapse everything into a single wave.
    Unordered,
}
