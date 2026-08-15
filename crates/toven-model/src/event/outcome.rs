//! [`OutcomeSummary`] — the generic, item-based terminal summary.
//!
//! Unlike the execution-unit-centric [`RunStats`](crate::RunStats), this summary
//! counts *items of work* in the abstract — an execution unit, a released
//! module, a gated coverage module — so the run, release, and coverage verbs
//! share one terminal shape. It owns the failure verdict: the process exit is
//! derived solely from [`OutcomeSummary::has_failures`], never from an
//! individual event.

use serde::{Deserialize, Serialize};

use super::RunStats;

/// A generic, item-based summary of one run's terminal outcome.
///
/// Each counter is a count of *items processed*, whatever the item is for the
/// verb (an execution unit, a released module, a coverage-gated module). The
/// unit path feeds it via [`RunStats::outcome`]; the progressive release and
/// coverage paths feed it directly. Because a single summary owns the verdict,
/// `has_failures` is the one input to exit-code derivation for every verb.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct OutcomeSummary {
    /// Items considered by the run (the denominator).
    pub processed: usize,
    /// Items that completed successfully (includes satisfied-by-cache units and
    /// already-up-to-date modules — work that reached a good terminal state).
    pub succeeded: usize,
    /// Items that failed. Any non-zero value makes [`has_failures`] true and
    /// drives a non-zero process exit.
    ///
    /// [`has_failures`]: OutcomeSummary::has_failures
    pub failed: usize,
    /// Items intentionally not completed but not failures (cancelled under
    /// fail-fast, advisory/excluded coverage, a module with nothing to stage).
    pub skipped: usize,
}

impl OutcomeSummary {
    /// Whether any item failed — the sole input to exit-code derivation.
    ///
    /// Kept deliberately narrow (only `failed`): skipped items are not failures,
    /// mirroring [`RunStats::has_failures`], so an all-cached run and a
    /// fail-fast-cancelled-but-otherwise-healthy run both report success here.
    #[must_use]
    pub const fn has_failures(&self) -> bool {
        self.failed > 0
    }
}

impl RunStats {
    /// Project these unit-centric counters onto the generic item summary.
    ///
    /// The mapping preserves the failure verdict: `outcome().has_failures()`
    /// equals [`RunStats::has_failures`], so routing the unit path's exit
    /// through the shared [`OutcomeSummary`] cannot change an existing run's
    /// outcome. Cache hits and successful runs count as `succeeded`; the four
    /// failure counters fold into `failed`; a fail-fast cancellation is
    /// `skipped`.
    #[must_use]
    pub const fn outcome(&self) -> OutcomeSummary {
        OutcomeSummary {
            processed: self.planned_units,
            succeeded: self.ran_units + self.cached_units,
            failed: self.failed_units
                + self.blocked_units
                + self.failed_readiness_units
                + self.timed_out_units,
            skipped: self.cancelled_units,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::OutcomeSummary;
    use crate::RunStats;

    fn round_trip(summary: &OutcomeSummary) {
        let json = serde_json::to_string(summary).expect("serializes");
        let back: OutcomeSummary = serde_json::from_str(&json).expect("round-trips");
        assert_eq!(summary, &back);
    }

    #[test]
    fn outcome_summary_round_trips() {
        round_trip(&OutcomeSummary::default());
        round_trip(&OutcomeSummary {
            processed: 5,
            succeeded: 3,
            failed: 1,
            skipped: 1,
        });
    }

    #[test]
    fn has_failures_tracks_the_failed_count_only() {
        assert!(!OutcomeSummary::default().has_failures());
        assert!(
            !OutcomeSummary {
                processed: 2,
                succeeded: 1,
                failed: 0,
                skipped: 1,
            }
            .has_failures(),
            "a skipped item is not a failure"
        );
        assert!(
            OutcomeSummary {
                processed: 2,
                succeeded: 1,
                failed: 1,
                skipped: 0,
            }
            .has_failures()
        );
    }

    #[test]
    fn run_stats_outcome_preserves_the_failure_verdict() {
        let mut clean = RunStats::new(3);
        clean.ran_units = 2;
        clean.cached_units = 1;
        assert_eq!(clean.outcome().has_failures(), clean.has_failures());
        assert!(!clean.outcome().has_failures());
        assert_eq!(clean.outcome().succeeded, 3);
        assert_eq!(clean.outcome().processed, 3);

        for mutate in [
            |s: &mut RunStats| s.failed_units = 1,
            |s: &mut RunStats| s.blocked_units = 1,
            |s: &mut RunStats| s.failed_readiness_units = 1,
            |s: &mut RunStats| s.timed_out_units = 1,
        ] {
            let mut stats = RunStats::new(2);
            mutate(&mut stats);
            assert!(stats.outcome().has_failures());
            assert_eq!(stats.outcome().has_failures(), stats.has_failures());
        }
    }

    #[test]
    fn a_fail_fast_cancellation_is_skipped_not_failed() {
        let mut stats = RunStats::new(2);
        stats.ran_units = 1;
        stats.cancelled_units = 1;
        let outcome = stats.outcome();
        assert_eq!(outcome.skipped, 1);
        assert!(!outcome.has_failures());
    }
}
