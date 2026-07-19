//! Closed union of terminal states a unit can reach.

use serde::{Deserialize, Serialize};

/// Final status of an [`ExecutionUnit`](crate::ExecutionUnit) in a run.
///
/// One closed union covering both normal and persistent units, carried by
/// [`Event::UnitFinished`](crate::Event::UnitFinished).
#[derive(Debug, Clone, Copy, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UnitStatus {
    /// Skipped because it was a cache hit (counts as satisfied).
    Cached,
    /// Ran to successful completion.
    Succeeded,
    /// Ran and failed.
    Failed,
    /// Never ran because an upstream dependency failed (fail-closed).
    Blocked,
    /// Not run, or interrupted in flight, because the run aborted early under
    /// fail-fast after a different unit failed. Distinct from `Blocked` (no
    /// dependency relationship is implied) and not itself a failure.
    Cancelled,
    /// Persistent unit reached readiness and is held in the background.
    Ready,
    /// Persistent unit shut down cleanly after its dependents drained.
    TornDown,
    /// Persistent unit never became ready within its readiness timeout.
    FailedReadiness,
    /// Ran past its per-unit execution timeout and was cooperatively cancelled
    /// (a failure). Distinct from [`FailedReadiness`](Self::FailedReadiness),
    /// which is a persistent unit's readiness-probe timeout, not a normal
    /// unit's execution bound.
    TimedOut,
}

impl UnitStatus {
    /// Whether this status represents a failure for exit-code purposes.
    #[must_use]
    pub const fn is_failure(self) -> bool {
        matches!(
            self,
            Self::Failed | Self::Blocked | Self::FailedReadiness | Self::TimedOut
        )
    }
}
