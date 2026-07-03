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
    /// A run finished; carries the aggregated summary.
    ///
    /// The process exit code is *not* a field: it is fully derived from
    /// `summary` by the single owner (`toven-cli`'s `exit_code`), so the event
    /// vocabulary keeps one source of truth and cannot desync a stored exit from
    /// the counters: the exit is derived from the summary.
    RunFinished {
        /// Aggregated run statistics.
        summary: RunStats,
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

    // ---- DIAGNOSTIC level ----
    /// A non-fatal diagnostic surfaced during a run (e.g. a canonical ecosystem
    /// whose driver is absent was skipped). Advisory only — it never changes the
    /// run outcome or exit code, but is always shown so the skip is not silent.
    Warning {
        /// Human-readable, actionable warning text.
        message: String,
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

    // ---- WATCH level ----
    /// Watch mode began observing the workspace; each subsequent change batch
    /// drives one PLAN→APPLY run.
    WatchStarted {
        /// Trailing-edge debounce window, in milliseconds.
        debounce_ms: u64,
    },
    /// A debounced batch of changes triggered a rerun. The listed paths are
    /// workspace-relative and already filtered to tracked, non-ignored files.
    WatchTriggered {
        /// The changed paths that triggered this iteration.
        paths: Vec<String>,
    },
    /// The watcher dropped events (typically a queue overflow), so the change
    /// list was incomplete; watch mode re-evaluated the whole watched scope
    /// instead of trusting the partial paths.
    WatchRescan,
    /// Watch mode stopped (cancelled by the operator or a terminating signal).
    WatchStopped,
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
        round_trip(&Event::Warning {
            message: "ecosystem 'go' skipped: no driver installed".into(),
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
        });
        round_trip(&Event::WatchStarted { debounce_ms: 200 });
        round_trip(&Event::WatchTriggered {
            paths: vec!["crates/toven-model/src/lib.rs".into()],
        });
        round_trip(&Event::WatchRescan);
        round_trip(&Event::WatchStopped);
    }
}
