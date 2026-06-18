//! The closed, typed [`Event`] vocabulary spanning a run's four levels.

use serde::{Deserialize, Serialize};

use super::{Phase, RunStats, UnitStatus};
use crate::plan::CacheVerdict;

/// The closed, typed event vocabulary spanning a run's four levels.
///
/// Both the PLAN and APPLY halves emit; `--explain`/dry-run is a PLAN-only
/// projection of the same stream.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum Event {
    // ---- RUN level ----
    /// A run began.
    RunStarted {
        /// Stable run identifier.
        run_id: String,
        /// What the run is doing (e.g. `test`, `build`, `release`).
        intent: String,
        /// Project name.
        project: String,
    },
    /// A run finished; carries the summary and process exit code.
    RunFinished {
        /// Aggregated run statistics.
        summary: RunStats,
        /// Process exit code derived from the summary.
        exit: i32,
    },

    // ---- PHASE level ----
    /// A PLAN phase started.
    PhaseStarted {
        /// The phase.
        phase: Phase,
    },
    /// A PLAN phase finished.
    PhaseFinished {
        /// The phase.
        phase: Phase,
    },

    // ---- PLAN level ----
    /// The immutable plan was prepared (the PLAN→APPLY boundary).
    PlanPrepared {
        /// Number of ready waves.
        waves: usize,
        /// Number of execution units.
        units: usize,
    },
    /// A per-unit cache verdict was decided.
    CacheDecided {
        /// Unit the verdict applies to.
        unit_id: String,
        /// The verdict.
        verdict: CacheVerdict,
    },

    // ---- UNIT level ----
    /// A unit started.
    UnitStarted {
        /// Unit identifier.
        unit_id: String,
    },
    /// A persistent unit reached readiness and is held.
    UnitReady {
        /// Unit identifier.
        unit_id: String,
    },
    /// A unit reached a terminal state.
    UnitFinished {
        /// Unit identifier.
        unit_id: String,
        /// Final status.
        status: UnitStatus,
    },
}

#[cfg(test)]
mod tests {
    use super::{Event, Phase, RunStats, UnitStatus};
    use crate::plan::CacheVerdict;

    fn round_trip(event: &Event) {
        let json = serde_json::to_string(event).unwrap();
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(event, &back);
    }

    #[test]
    fn events_round_trip() {
        round_trip(&Event::RunStarted {
            run_id: "r1".into(),
            intent: "test".into(),
            project: "toven".into(),
        });
        round_trip(&Event::PhaseStarted {
            phase: Phase::Discover,
        });
        round_trip(&Event::CacheDecided {
            unit_id: "u1".into(),
            verdict: CacheVerdict::Hit,
        });
        round_trip(&Event::UnitFinished {
            unit_id: "u1".into(),
            status: UnitStatus::Succeeded,
        });
        round_trip(&Event::RunFinished {
            summary: RunStats::new(3),
            exit: 0,
        });
    }
}
