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
    /// Persistent unit reached readiness and is held in the background.
    Ready,
    /// Persistent unit shut down cleanly after its dependents drained.
    TornDown,
    /// Persistent unit never became ready within its readiness timeout.
    FailedReadiness,
}

impl UnitStatus {
    /// Whether this status represents a failure for exit-code purposes.
    #[must_use]
    pub const fn is_failure(self) -> bool {
        matches!(self, Self::Failed | Self::Blocked | Self::FailedReadiness)
    }
}
