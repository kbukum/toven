//! The closed, typed [`Event`] vocabulary spanning a run's four levels.

use serde::{Deserialize, Serialize};

use super::{Phase, RunStats, UnitStatus};
use crate::plan::CacheVerdict;
use crate::tool::ToolStatus;

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
    /// vocabulary keeps one source of truth and cannot desync a stored exit
    /// from the counters: the exit is derived from the summary.
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
    /// whose driver is absent was skipped). Advisory only — it never changes
    /// the run outcome or exit code, but is always shown so the skip is not
    /// silent.
    Warning {
        /// Human-readable, actionable warning text.
        message: String,
    },
    /// The changed-path selection could not attribute one or more paths to a
    /// module, so every module was activated (fail-closed). Advisory: it
    /// explains *why* a full run was planned and never changes the outcome.
    /// Empty `paths` is never emitted — the event fires only when a full
    /// activation was forced.
    FullActivation {
        /// The changed paths that no module or workspace could claim.
        paths: Vec<String>,
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
        /// Process exit code, populated only for a unit that ran a command and
        /// exited non-zero. `None` for a success or a terminal state that never
        /// ran a process (cached, blocked, cancelled, torn-down). The unit's
        /// captured stdout/stderr is surfaced on the separate raw-output
        /// channel, so this names the failure without duplicating that stream.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
    },

    // ---- DOCTOR level ----
    /// A required tool for the resolved task graph was audited: present (with an
    /// optional version) or missing. The `doctor` verb emits one per unique
    /// tool through the same reporter sinks a run uses.
    ToolAudited {
        /// The probe's human-readable label (e.g. `"cargo"`).
        label: String,
        /// The program that was probed (`argv[0]`).
        program: String,
        /// Whether the tool is present (and its version) or missing.
        status: ToolStatus,
    },
    /// The `doctor` audit finished. Advisory summary: the process exit is
    /// derived from `missing` by the CLI (non-zero when any required tool is
    /// absent), so the count is the single source of truth.
    DoctorFinished {
        /// Total tools checked across the resolved task graph.
        checked: usize,
        /// How many of them were missing.
        missing: usize,
    },

    // ---- WATCH level ----
    /// Watch mode began observing the workspace; each subsequent change batch
    /// drives one PLAN→APPLY run.
    WatchStarted {
        /// Trailing-edge debounce window, in milliseconds.
        debounce_ms: u64,
    },
    /// A debounced batch of changes triggered a rerun. The listed paths are
    /// workspace-relative, with paths inside `.git` and paths ignored by the
    /// root repo already dropped.
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
    use crate::tool::ToolStatus;

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
        round_trip(&Event::FullActivation {
            paths: vec!["toven.toml".into(), "README.md".into()],
        });
        round_trip(&Event::CacheDecided {
            unit_id: "u1".into(),
            verdict: CacheVerdict::Hit,
        });
        round_trip(&Event::UnitFinished {
            unit_id: "u1".into(),
            status: UnitStatus::Succeeded,
            exit_code: None,
        });
        round_trip(&Event::UnitFinished {
            unit_id: "u2".into(),
            status: UnitStatus::Failed,
            exit_code: Some(2),
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
        round_trip(&Event::ToolAudited {
            label: "cargo".into(),
            program: "cargo".into(),
            status: ToolStatus::Present {
                version: Some("cargo 1.94.0".into()),
            },
        });
        round_trip(&Event::ToolAudited {
            label: "mdbook".into(),
            program: "mdbook".into(),
            status: ToolStatus::Missing,
        });
        round_trip(&Event::DoctorFinished {
            checked: 2,
            missing: 1,
        });
    }
}
