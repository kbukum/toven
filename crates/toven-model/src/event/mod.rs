//! Observability vocabulary: the closed [`Event`] stream the engine emits.
//!
//! The engine emits typed `Event`s only; it never formats. A `Reporter` sink
//! (defined with the ports) renders them. Because the stream is typed and
//! serializable it also crosses the out-of-process driver boundary as
//! diagnostics. Raw child output is deliberately *not* per-line vocabulary — it
//! flows through the separate, coarse [`UnitOutput`] channel so high-throughput
//! build output never pays per-line (de)serialization.

mod stats;
mod status;

pub use stats::RunStats;
pub use status::UnitStatus;

use serde::{Deserialize, Serialize};

use crate::plan::CacheVerdict;

/// A named phase of the pure PLAN half, reported for progress.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// Parse + structurally validate config.
    Load,
    /// Configure per-ecosystem adapters.
    Configure,
    /// Federated discovery across loaded ecosystems.
    Discover,
    /// Build + validate the dependency graph.
    Graph,
    /// Map changes to the active module set.
    Affected,
    /// Resolve toolchain identity for active workspaces.
    Toolchain,
    /// Compute the federated wave sequence + cache verdicts.
    Schedule,
}

/// The output stream a [`UnitOutput`] chunk came from.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputStream {
    /// Standard output.
    Stdout,
    /// Standard error.
    Stderr,
}

/// A chunk of raw child output, attributed to a unit.
///
/// Carried on a separate channel from [`Event`] (Decision C): coarse-grained and
/// not part of the typed event union, so build output is cheap to route.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
pub struct UnitOutput {
    /// Unit the output belongs to.
    pub unit_id: String,
    /// Stream the bytes came from.
    pub stream: OutputStream,
    /// Raw bytes (not interpreted as UTF-8).
    pub bytes: Vec<u8>,
}

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
    use super::{Event, OutputStream, Phase, RunStats, UnitOutput, UnitStatus};
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

    #[test]
    fn unit_output_round_trips() {
        let output = UnitOutput {
            unit_id: "u1".into(),
            stream: OutputStream::Stderr,
            bytes: b"hello".to_vec(),
        };
        let json = serde_json::to_string(&output).unwrap();
        let back: UnitOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(output, back);
    }
}
