//! [`HumanReporter`] — the terminal-facing Event-stream sink.

use std::io::{self, Write};

use rskit_cli::OutputKV;
use rskit_errors::{AppError, AppResult};
use toven_model::{CacheVerdict, Event, Phase, RunStats, UnitStatus};
use toven_ports::Reporter;

use super::exit::exit_code;
use crate::flags::Verbosity;

/// A [`Reporter`] that renders the Event stream as readable terminal text.
///
/// Run/phase/plan/unit levels become labeled progress lines and the terminal
/// `RunFinished` summary becomes an [`OutputKV`] block (rskit-cli). The same
/// renderer serves a real run and an `--explain`/dry-run PLAN-only projection —
/// it simply renders whatever subset of the stream it is given, so there is no
/// bespoke plan-dump. A [`Verbosity`] level filters how much of the stream is
/// shown (quiet collapses to the run summary; verbose adds the per-phase,
/// cache-decision, and unit-lifecycle detail). Generic over the writer for
/// testability; [`HumanReporter::stdout`] binds the process stdout.
pub struct HumanReporter<W: Write> {
    writer: W,
    level: Verbosity,
}

impl<W: Write> HumanReporter<W> {
    /// Create a reporter that renders events to `writer` at `level`.
    pub const fn new(writer: W, level: Verbosity) -> Self {
        Self { writer, level }
    }

    /// Whether an event is rendered at the given verbosity level.
    ///
    /// Run start/finish always render; the plan line and terminal per-unit
    /// results are suppressed only at [`Verbosity::Quiet`]; and the per-phase,
    /// cache-decision, and intermediate unit-lifecycle lines render only at
    /// [`Verbosity::Verbose`].
    const fn renders(level: Verbosity, event: &Event) -> bool {
        match event {
            Event::RunStarted { .. } | Event::RunFinished { .. } => true,
            Event::PlanPrepared { .. } | Event::UnitFinished { .. } => {
                !matches!(level, Verbosity::Quiet)
            }
            Event::PhaseStarted { .. }
            | Event::PhaseFinished { .. }
            | Event::CacheDecided { .. }
            | Event::UnitStarted { .. }
            | Event::UnitReady { .. } => matches!(level, Verbosity::Verbose),
        }
    }

    /// Consume the reporter and recover the underlying writer.
    ///
    /// Test-only: the production stdout reporter is write-only; recovering the
    /// writer exists solely so unit tests can assert the rendered bytes.
    #[cfg(test)]
    pub fn into_inner(self) -> W {
        self.writer
    }

    fn write_line(&mut self, line: &str) -> AppResult<()> {
        writeln!(self.writer, "{line}").map_err(AppError::internal)?;
        // Flush each progress line so redirected/piped stdout (block-buffered)
        // surfaces progress promptly instead of in deferred bursts.
        self.writer.flush().map_err(AppError::internal)
    }

    fn write_summary(&mut self, summary: &RunStats) -> AppResult<()> {
        let mut kv = OutputKV::new();
        kv.add("planned", summary.planned_units.to_string());
        kv.add("ran", summary.ran_units.to_string());
        kv.add("cached", summary.cached_units.to_string());
        kv.add("failed", summary.failed_units.to_string());
        kv.add("blocked", summary.blocked_units.to_string());
        kv.add("cancelled", summary.cancelled_units.to_string());
        kv.add(
            "failed-readiness",
            summary.failed_readiness_units.to_string(),
        );
        if let Some(duration_ms) = summary.duration_ms {
            kv.add("duration-ms", duration_ms.to_string());
        }
        // Surfaced only when output was actually dropped, so honest loss is
        // reported without cluttering the common (lossless) summary.
        if summary.dropped_output_chunks > 0 {
            kv.add("dropped-output", summary.dropped_output_chunks.to_string());
        }
        // The displayed exit is derived from the summary by the single owner, so
        // it can never disagree with the actual process exit (event-report C).
        kv.add("exit", exit_code(summary).as_i32().to_string());
        write!(self.writer, "summary\n{kv}").map_err(AppError::internal)?;
        // Flush the final summary so a piped/redirected consumer receives it
        // promptly and it is not lost in a buffer on an abrupt exit.
        self.writer.flush().map_err(AppError::internal)
    }
}

impl HumanReporter<io::Stdout> {
    /// Create a reporter writing human-readable text to process stdout at `level`.
    #[must_use]
    pub fn stdout(level: Verbosity) -> Self {
        Self::new(io::stdout(), level)
    }
}

impl<W: Write + Send> Reporter for HumanReporter<W> {
    fn emit(&mut self, event: &Event) -> AppResult<()> {
        if !Self::renders(self.level, event) {
            return Ok(());
        }
        match event {
            Event::RunStarted {
                run_id,
                intent,
                project,
            } => self.write_line(&format!("run {run_id}: {intent} on {project}")),
            Event::RunFinished { summary } => self.write_summary(summary),
            Event::PhaseStarted { phase } => {
                self.write_line(&format!("  phase {}: started", phase_label(*phase)))
            }
            Event::PhaseFinished { phase } => {
                self.write_line(&format!("  phase {}: done", phase_label(*phase)))
            }
            Event::PlanPrepared { waves, units } => {
                self.write_line(&format!("plan: {units} units in {waves} waves"))
            }
            Event::CacheDecided { unit_id, verdict } => {
                self.write_line(&format!("  cache {unit_id}: {}", verdict_label(*verdict)))
            }
            Event::UnitStarted { unit_id } => self.write_line(&format!("  start {unit_id}")),
            Event::UnitReady { unit_id } => self.write_line(&format!("  ready {unit_id}")),
            Event::UnitFinished { unit_id, status } => {
                self.write_line(&format!("  {} {unit_id}", status_label(*status)))
            }
        }
    }
}

const fn phase_label(phase: Phase) -> &'static str {
    match phase {
        Phase::Load => "load",
        Phase::Configure => "configure",
        Phase::Discover => "discover",
        Phase::Graph => "graph",
        Phase::Affected => "affected",
        Phase::Toolchain => "toolchain",
        Phase::Schedule => "schedule",
    }
}

const fn verdict_label(verdict: CacheVerdict) -> &'static str {
    match verdict {
        CacheVerdict::Hit => "hit",
        CacheVerdict::Miss => "miss",
        CacheVerdict::Disabled => "disabled",
        CacheVerdict::Forced => "forced",
    }
}

const fn status_label(status: UnitStatus) -> &'static str {
    match status {
        UnitStatus::Cached => "cached",
        UnitStatus::Succeeded => "ok",
        UnitStatus::Failed => "failed",
        UnitStatus::Blocked => "blocked",
        UnitStatus::Cancelled => "cancelled",
        UnitStatus::Ready => "ready",
        UnitStatus::TornDown => "torn-down",
        UnitStatus::FailedReadiness => "failed-readiness",
    }
}

#[cfg(test)]
mod tests {
    use toven_model::{CacheVerdict, Event, Phase, RunStats, UnitStatus};
    use toven_ports::Reporter;

    use super::HumanReporter;
    use crate::flags::Verbosity;

    fn render(events: &[Event]) -> String {
        render_at(Verbosity::Verbose, events)
    }

    fn render_at(level: Verbosity, events: &[Event]) -> String {
        let mut reporter = HumanReporter::new(Vec::new(), level);
        for event in events {
            reporter.emit(event).expect("emit");
        }
        String::from_utf8(reporter.into_inner()).expect("utf8")
    }

    #[test]
    fn explain_projection_renders_plan_only_stream() {
        // `--explain` / dry-run emits the PLAN-side events + RunFinished, never
        // entering APPLY: no UnitStarted/UnitFinished. Asserted as an exact
        // golden so a layout/ordering regression in the summary block fails.
        let mut summary = RunStats::new(2);
        summary.cached_units = 1;
        summary.cache_hits = 1;
        summary.cache_misses = 1;
        let events = vec![
            Event::RunStarted {
                run_id: "r1".into(),
                intent: "test".into(),
                project: "toven".into(),
            },
            Event::PhaseStarted {
                phase: Phase::Schedule,
            },
            Event::PhaseFinished {
                phase: Phase::Schedule,
            },
            Event::PlanPrepared { waves: 1, units: 2 },
            Event::CacheDecided {
                unit_id: "rust:errors#test".into(),
                verdict: CacheVerdict::Hit,
            },
            Event::CacheDecided {
                unit_id: "rust:core#test".into(),
                verdict: CacheVerdict::Miss,
            },
            Event::RunFinished { summary },
        ];
        let output = render(&events);

        let expected = "\
run r1: test on toven
  phase schedule: started
  phase schedule: done
plan: 2 units in 1 waves
  cache rust:errors#test: hit
  cache rust:core#test: miss
summary
           planned:  2
               ran:  0
            cached:  1
            failed:  0
           blocked:  0
         cancelled:  0
  failed-readiness:  0
              exit:  0
";
        assert_eq!(output, expected);
        // The PLAN-only projection carries no unit-exec lines.
        assert!(!output.contains("start "), "no unit-exec lines: {output}");
        assert!(!output.contains(" ok "), "no unit-exec lines: {output}");
    }

    #[test]
    fn renders_full_run_unit_lifecycle() {
        let mut summary = RunStats::new(1);
        summary.ran_units = 1;
        let events = vec![
            Event::UnitStarted {
                unit_id: "u1".into(),
            },
            Event::UnitFinished {
                unit_id: "u1".into(),
                status: UnitStatus::Succeeded,
            },
            Event::RunFinished { summary },
        ];
        let output = render(&events);

        let expected = "  start u1\n  ok u1\nsummary\n           planned:  1\n               ran:  1\n            cached:  0\n            failed:  0\n           blocked:  0\n         cancelled:  0\n  failed-readiness:  0\n              exit:  0\n";
        assert_eq!(output, expected);
    }

    #[test]
    fn failed_run_summary_derives_non_zero_exit_from_counters() {
        // The summary's `exit` line is derived from the counters by the single
        // owner, never trusted from the event — a failure forces a non-zero exit.
        let mut summary = RunStats::new(1);
        summary.failed_units = 1;
        let output = render(&[Event::RunFinished { summary }]);

        let expected = "\
summary
           planned:  1
               ran:  0
            cached:  0
            failed:  1
           blocked:  0
         cancelled:  0
  failed-readiness:  0
              exit:  1
";
        assert_eq!(output, expected);
    }

    #[test]
    fn summary_renders_optional_duration_when_present() {
        let mut summary = RunStats::new(1);
        summary.duration_ms = Some(42);
        let output = render(&[Event::RunFinished { summary }]);
        assert!(
            output.contains("       duration-ms:  42\n"),
            "duration line missing: {output}"
        );
    }

    #[test]
    fn every_unit_status_renders_its_label() {
        // Locks the label for each variant so a wrong/duplicated mapping fails;
        // pairs with the compile-time exhaustiveness of `status_label`.
        let cases = [
            (UnitStatus::Cached, "  cached u\n"),
            (UnitStatus::Succeeded, "  ok u\n"),
            (UnitStatus::Failed, "  failed u\n"),
            (UnitStatus::Blocked, "  blocked u\n"),
            (UnitStatus::Cancelled, "  cancelled u\n"),
            (UnitStatus::Ready, "  ready u\n"),
            (UnitStatus::TornDown, "  torn-down u\n"),
            (UnitStatus::FailedReadiness, "  failed-readiness u\n"),
        ];
        for (status, expected) in cases {
            let output = render(&[Event::UnitFinished {
                unit_id: "u".into(),
                status,
            }]);
            assert_eq!(output, expected, "status {status:?}");
        }
    }

    #[test]
    fn renders_persistent_lifecycle_lines() {
        let events = vec![
            Event::UnitReady {
                unit_id: "srv".into(),
            },
            Event::UnitFinished {
                unit_id: "srv".into(),
                status: UnitStatus::TornDown,
            },
        ];
        let output = render(&events);
        assert_eq!(output, "  ready srv\n  torn-down srv\n");
    }

    /// The full stream every level test renders a subset of.
    fn full_stream() -> Vec<Event> {
        vec![
            Event::RunStarted {
                run_id: "r1".into(),
                intent: "build".into(),
                project: "toven".into(),
            },
            Event::PhaseStarted {
                phase: Phase::Schedule,
            },
            Event::PhaseFinished {
                phase: Phase::Schedule,
            },
            Event::PlanPrepared { waves: 1, units: 1 },
            Event::CacheDecided {
                unit_id: "rust:core#build".into(),
                verdict: CacheVerdict::Miss,
            },
            Event::UnitStarted {
                unit_id: "rust:core#build".into(),
            },
            Event::UnitFinished {
                unit_id: "rust:core#build".into(),
                status: UnitStatus::Succeeded,
            },
            Event::RunFinished {
                summary: RunStats::new(1),
            },
        ]
    }

    #[test]
    fn quiet_collapses_to_the_run_lines_and_summary() {
        let output = render_at(Verbosity::Quiet, &full_stream());
        // Run start + summary survive; everything in between is suppressed.
        assert!(output.starts_with("run r1: build on toven\n"), "{output}");
        assert!(output.contains("summary\n"), "{output}");
        for noise in ["phase ", "plan:", "cache ", "  start ", "  ok "] {
            assert!(!output.contains(noise), "quiet leaked {noise:?}: {output}");
        }
    }

    #[test]
    fn normal_shows_plan_and_terminal_results_but_not_intermediate_noise() {
        let output = render_at(Verbosity::Normal, &full_stream());
        assert!(output.contains("run r1: build on toven\n"), "{output}");
        assert!(output.contains("plan: 1 units in 1 waves\n"), "{output}");
        assert!(output.contains("  ok rust:core#build\n"), "{output}");
        assert!(output.contains("summary\n"), "{output}");
        // Per-phase, cache-decision, and unit-start lines are verbose-only.
        for noise in ["phase ", "cache ", "  start "] {
            assert!(!output.contains(noise), "normal leaked {noise:?}: {output}");
        }
    }

    #[test]
    fn verbose_renders_every_event() {
        let output = render_at(Verbosity::Verbose, &full_stream());
        for line in [
            "run r1: build on toven\n",
            "  phase schedule: started\n",
            "plan: 1 units in 1 waves\n",
            "  cache rust:core#build: miss\n",
            "  start rust:core#build\n",
            "  ok rust:core#build\n",
            "summary\n",
        ] {
            assert!(output.contains(line), "verbose missing {line:?}: {output}");
        }
    }
}
