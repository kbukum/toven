//! The closed, typed [`Event`] vocabulary spanning a run's four levels.

use serde::{Deserialize, Serialize};

use super::{CoverageMeasurement, CoverageVerdict, Phase, RunStats, UnitStatus};
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

    // ---- RELEASE level ----
    /// A per-module release decision is **about to run** its slow I/O (baseline
    /// resolution, change detection, registry lookup). Advisory progress only:
    /// it never changes the run outcome or exit code and is safe to emit before
    /// any mutation, filling the otherwise-silent gap so an operator sees
    /// module-by-module motion. It precedes that module's settled
    /// [`ModuleReleaseResolved`](Self::ModuleReleaseResolved) decision.
    ModuleReleaseExamining {
        /// Module whose decision I/O is starting (its canonical key).
        module: String,
    },
    /// A per-module release *decision*, resolved from the plan **before** any
    /// mutation. Safe to stream immediately, per module, in deterministic plan
    /// order — it is exactly what `release plan` and the bare-command preview
    /// project, and `--dry-run` emits only these. Distinct from
    /// [`ModuleReleaseStaged`](Self::ModuleReleaseStaged): a decision is a
    /// prediction, never a committed fact, so a later whole-run restore never
    /// contradicts an already-emitted decision.
    ModuleReleaseResolved {
        /// Module the decision is for (its canonical key).
        module: String,
        /// Version the module's manifest currently declares.
        current_version: String,
        /// Version to release, when the module receives an own-version bump.
        /// `None` when only a dependency floor moves or the module is already
        /// up to date, so a decision with no own-version change is not rendered
        /// as a bogus transition.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        planned_version: Option<String>,
        /// Canonical bump-level name (`patch`/`minor`/`major`) applied to reach
        /// the planned version.
        level: String,
        /// Canonical reason name for the decision (e.g. `changed`,
        /// `initial-release`, `dependency-cascade`).
        reason: String,
        /// Planned release tag under the module's tag grammar, when the module
        /// tags. Omitted for a tag-less ecosystem.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tag: Option<String>,
        /// The module's publish disposition (e.g. the registry action), when
        /// applicable. Omitted when the verb does not publish.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        publication: Option<String>,
        /// Whether the planned version is already at/above the registry (or,
        /// offline, the release tag), making a real publish a reported no-op.
        up_to_date: bool,
    },
    /// A per-module release *commit*, emitted **only after** the transactional
    /// side effect for that module has actually landed.
    ///
    /// Because it fires post-commit, a mid-transaction failure that triggers the
    /// whole-run restore never leaves an emitted-but-rolled-back "staged" event
    /// — the fail-closed guarantee holds by construction. A module that stages
    /// nothing (a tag-only ecosystem with no rolled changelog) emits no staged
    /// event, matching the reported staging truth.
    ModuleReleaseStaged {
        /// Module the commit is for (its canonical key).
        module: String,
        /// The version actually cut for the module.
        new_version: String,
        /// Manifest paths rewritten for the module (workspace-relative).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        manifests: Vec<String>,
        /// Changelog path rolled for the module, when one was rolled.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        changelog: Option<String>,
        /// The tag created for the module, for the tag/publish tail. Omitted
        /// when this commit created no tag.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tag: Option<String>,
    },

    // ---- COVERAGE level ----
    /// A per-module coverage verdict, emitted as each module's aggregation
    /// completes. The verdict feeds the terminal
    /// [`OutcomeSummary`](crate::OutcomeSummary), never a per-event exit.
    ModuleCoverageFinished {
        /// Module the verdict is for (its canonical key).
        module: String,
        /// Measured-vs-threshold values, one per gated or measured dimension.
        measurements: Vec<CoverageMeasurement>,
        /// The module's overall verdict.
        verdict: CoverageVerdict,
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
    use super::{CoverageMeasurement, CoverageVerdict, Event, Phase, RunStats, UnitStatus};
    use crate::event::CoverageMetric;
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

    #[test]
    fn release_and_coverage_events_round_trip() {
        // Advisory per-module progress emitted before the slow decision I/O.
        round_trip(&Event::ModuleReleaseExamining {
            module: "core".into(),
        });
        // Full decision with every optional field populated.
        round_trip(&Event::ModuleReleaseResolved {
            module: "core".into(),
            current_version: "1.2.0".into(),
            planned_version: Some("1.3.0".into()),
            level: "minor".into(),
            reason: "changed".into(),
            tag: Some("core-v1.3.0".into()),
            publication: Some("publish".into()),
            up_to_date: false,
        });
        // A decision with no own-version bump omits the optional fields.
        round_trip(&Event::ModuleReleaseResolved {
            module: "leaf".into(),
            current_version: "0.4.1".into(),
            planned_version: None,
            level: "patch".into(),
            reason: "dependency-cascade".into(),
            tag: None,
            publication: None,
            up_to_date: true,
        });
        round_trip(&Event::ModuleReleaseStaged {
            module: "core".into(),
            new_version: "1.3.0".into(),
            manifests: vec!["crates/core/Cargo.toml".into()],
            changelog: Some("crates/core/CHANGELOG.md".into()),
            tag: Some("core-v1.3.0".into()),
        });
        // A tag-only stage that rewrote nothing keeps the empty collections out
        // of the machine projection.
        round_trip(&Event::ModuleReleaseStaged {
            module: "leaf".into(),
            new_version: "0.4.2".into(),
            manifests: Vec::new(),
            changelog: None,
            tag: None,
        });
        round_trip(&Event::ModuleCoverageFinished {
            module: "core".into(),
            measurements: vec![
                CoverageMeasurement {
                    metric: CoverageMetric::Line,
                    measured: 9537,
                    threshold: Some(9000),
                    met: true,
                },
                CoverageMeasurement {
                    metric: CoverageMetric::ChangedLine,
                    measured: 8000,
                    threshold: None,
                    met: true,
                },
            ],
            verdict: CoverageVerdict::Passed,
        });
        round_trip(&Event::ModuleCoverageFinished {
            module: "leaf".into(),
            measurements: Vec::new(),
            verdict: CoverageVerdict::Failed,
        });
    }

    #[test]
    fn new_domain_variants_carry_stable_event_tags() {
        let examining = serde_json::to_value(Event::ModuleReleaseExamining {
            module: "core".into(),
        })
        .expect("serializes");
        assert_eq!(examining["event"], "module-release-examining");
        assert_eq!(examining["module"], "core");

        let resolved = serde_json::to_value(Event::ModuleReleaseResolved {
            module: "core".into(),
            current_version: "1.2.0".into(),
            planned_version: None,
            level: "patch".into(),
            reason: "changed".into(),
            tag: None,
            publication: None,
            up_to_date: true,
        })
        .expect("serializes");
        assert_eq!(resolved["event"], "module-release-resolved");
        // Absent optionals must not leak into the machine projection.
        assert!(resolved.get("planned_version").is_none());
        assert!(resolved.get("tag").is_none());
        assert!(resolved.get("publication").is_none());

        let staged = serde_json::to_value(Event::ModuleReleaseStaged {
            module: "leaf".into(),
            new_version: "0.4.2".into(),
            manifests: Vec::new(),
            changelog: None,
            tag: None,
        })
        .expect("serializes");
        assert_eq!(staged["event"], "module-release-staged");
        assert!(staged.get("manifests").is_none());

        let coverage = serde_json::to_value(Event::ModuleCoverageFinished {
            module: "core".into(),
            measurements: Vec::new(),
            verdict: CoverageVerdict::Excluded,
        })
        .expect("serializes");
        assert_eq!(coverage["event"], "module-coverage-finished");
        assert_eq!(coverage["verdict"], "excluded");
    }
}
