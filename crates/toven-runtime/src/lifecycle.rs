//! The per-unit lifecycle contract: [`UnitStatus`], [`UnitReport`],
//! [`RunSummary`], and the [`Progress`] streaming sink.

use rskit_errors::AppResult;

/// Terminal disposition of a unit, as the engine sees it.
///
/// This is the engine's minimal, verb-agnostic verdict — the domain-specific
/// detail (which version was bumped, which coverage dimensions failed, a child
/// exit code) rides in the typed outcome payload carried by [`UnitReport`], not
/// here.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum UnitStatus {
    /// The operation completed without failing.
    Succeeded,
    /// The operation ran and reported failure.
    Failed,
    /// An upstream failure blocked the unit before it ran.
    Blocked,
    /// The run aborted (fail-fast or external cancel) before the unit ran, or
    /// interrupted it in flight — not itself a failure.
    Cancelled,
}

impl UnitStatus {
    /// Whether this disposition drives a non-zero run outcome.
    #[must_use]
    pub const fn is_failure(self) -> bool {
        matches!(self, Self::Failed | Self::Blocked)
    }
}

/// The settled report for one unit: its id, terminal status, and — when it
/// actually ran — the typed per-family outcome payload.
///
/// `Blocked` and `Cancelled` units never ran, so they carry no `outcome`.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct UnitReport<T> {
    /// The unit this report is for.
    pub unit_id: String,
    /// The unit's terminal disposition.
    pub status: UnitStatus,
    /// The typed outcome, present only for units that ran (`Succeeded`/`Failed`).
    pub outcome: Option<T>,
}

/// Aggregated counts derived once from the streamed per-unit outcomes.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct RunSummary {
    /// Units in the plan.
    pub total: usize,
    /// Units that completed without failing.
    pub succeeded: usize,
    /// Units that ran and reported failure.
    pub failed: usize,
    /// Units blocked by an upstream failure.
    pub blocked: usize,
    /// Units cancelled (never run or interrupted) by an early abort.
    pub cancelled: usize,
}

impl RunSummary {
    /// Empty summary for a plan of `total` units.
    #[must_use]
    pub const fn new(total: usize) -> Self {
        Self {
            total,
            succeeded: 0,
            failed: 0,
            blocked: 0,
            cancelled: 0,
        }
    }

    /// Whether any unit failed or was blocked (drives a non-zero exit).
    #[must_use]
    pub const fn has_failures(&self) -> bool {
        self.failed > 0 || self.blocked > 0
    }
}

/// Streaming sink for the per-unit lifecycle.
///
/// The engine calls [`started`](Progress::started) when a unit is submitted and
/// [`settled`](Progress::settled) the instant it reaches a terminal state —
/// never after the whole set is computed. The consuming layer projects these
/// generic events onto its own event vocabulary and output sinks.
pub trait Progress<T>: Send {
    /// A unit has been submitted and is now running.
    ///
    /// # Errors
    /// Propagates any sink failure.
    fn started(&mut self, unit_id: &str) -> AppResult<()>;

    /// A unit has settled with the given report.
    ///
    /// # Errors
    /// Propagates any sink failure.
    fn settled(&mut self, report: &UnitReport<T>) -> AppResult<()>;
}

#[cfg(test)]
mod tests {
    use super::{RunSummary, UnitStatus};

    #[test]
    fn only_failed_and_blocked_are_failures() {
        assert!(UnitStatus::Failed.is_failure());
        assert!(UnitStatus::Blocked.is_failure());
        assert!(!UnitStatus::Succeeded.is_failure());
        assert!(!UnitStatus::Cancelled.is_failure());
    }

    #[test]
    fn summary_has_failures_tracks_failed_and_blocked() {
        let mut summary = RunSummary::new(3);
        assert!(!summary.has_failures());
        summary.blocked = 1;
        assert!(summary.has_failures());
    }
}
