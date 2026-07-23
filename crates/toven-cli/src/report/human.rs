//! [`HumanReporter`] — the terminal-facing Event-stream sink.

use std::borrow::Cow;
use std::io::{self, Write};

use rskit_cli::{OutputKV, Palette};
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
/// cache-decision, and unit-lifecycle detail). A [`Palette`] colorizes the
/// status labels and the summary's status line; it defaults to disabled (verbatim
/// text) so a piped or `--color never` run is byte-stable. Generic over the
/// writer for testability; [`HumanReporter::stderr`] binds the process stderr
/// (progress and status are diagnostics, so stdout stays reserved for the
/// machine projection).
pub struct HumanReporter<W: Write> {
    writer: W,
    level: Verbosity,
    palette: Palette,
}

impl<W: Write> HumanReporter<W> {
    /// Create a reporter that renders events to `writer` at `level`.
    ///
    /// Color is disabled until a [`Palette`] is attached with
    /// [`with_palette`](Self::with_palette), so the default rendering is
    /// verbatim text.
    #[must_use]
    pub const fn new(writer: W, level: Verbosity) -> Self {
        Self {
            writer,
            level,
            palette: Palette::new(false),
        }
    }

    /// Attach a resolved [`Palette`] so status labels and the summary status line
    /// are colorized; a disabled palette leaves the output verbatim.
    #[must_use]
    pub const fn with_palette(mut self, palette: Palette) -> Self {
        self.palette = palette;
        self
    }

    /// Whether an event is rendered at the given verbosity level.
    ///
    /// Run start/finish always render; the plan line and terminal per-unit
    /// results are suppressed only at [`Verbosity::Quiet`]; and the per-phase,
    /// cache-decision, and intermediate unit-lifecycle lines render only at
    /// [`Verbosity::Verbose`].
    const fn renders(level: Verbosity, event: &Event) -> bool {
        match event {
            Event::RunStarted { .. }
            | Event::RunFinished { .. }
            | Event::Warning { .. }
            | Event::FullActivation { .. }
            | Event::WatchStarted { .. }
            | Event::WatchTriggered { .. }
            | Event::WatchRescan
            | Event::WatchStopped => true,
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

    /// Colorize a terminal unit-status label by its outcome semantics: green
    /// for success, red for failure, yellow for blocked/cancelled, dim for a
    /// cache hit. A disabled palette returns the label verbatim.
    fn paint_status(&self, status: UnitStatus) -> Cow<'static, str> {
        let label = status_label(status);
        match status {
            UnitStatus::Succeeded | UnitStatus::Ready | UnitStatus::TornDown => {
                self.palette.success(label)
            }
            UnitStatus::Failed | UnitStatus::FailedReadiness | UnitStatus::TimedOut => {
                self.palette.error(label)
            }
            UnitStatus::Blocked | UnitStatus::Cancelled => self.palette.warn(label),
            UnitStatus::Cached => self.palette.dim(label),
        }
    }

    fn write_line(&mut self, line: &str) -> AppResult<()> {
        writeln!(self.writer, "{line}").map_err(AppError::internal)?;
        // Flush each progress line so redirected/piped stdout (block-buffered) surfaces
        // progress promptly instead of in deferred bursts.
        self.writer.flush().map_err(AppError::internal)
    }

    fn write_summary(&mut self, summary: &RunStats) -> AppResult<()> {
        // At default verbosity the failure-family counters collapse to only the
        // non-zero ones, so a clean run reads at a glance; `-v` keeps the full,
        // fixed-width table for a deterministic, scriptable-looking dump.
        let full = matches!(self.level, Verbosity::Verbose);
        let mut kv = OutputKV::new();
        kv.add("planned", summary.planned_units.to_string());
        kv.add("ran", summary.ran_units.to_string());
        kv.add("cached", summary.cached_units.to_string());
        for (label, count) in [
            ("failed", summary.failed_units),
            ("blocked", summary.blocked_units),
            ("cancelled", summary.cancelled_units),
            ("failed-readiness", summary.failed_readiness_units),
            ("timed-out", summary.timed_out_units),
        ] {
            if full || count > 0 {
                kv.add(label, count.to_string());
            }
        }
        if let Some(duration_ms) = summary.duration_ms {
            kv.add("duration-ms", duration_ms.to_string());
        }
        // Surfaced only when output was actually dropped, so honest loss is reported
        // without cluttering the common (lossless) summary.
        if summary.dropped_output_chunks > 0 {
            kv.add("dropped-output", summary.dropped_output_chunks.to_string());
        }
        // The displayed status is derived from the summary by the single owner, so it
        // can never disagree with the actual process exit (event-report C). Rendered as
        // a human word (`ok`/`failed`) rather than a bare exit number, colorized by
        // outcome (green success / red failure); a disabled palette leaves it verbatim.
        let ok = exit_code(summary).as_i32() == 0;
        let status_text = if ok { "ok" } else { "failed" };
        let status_value = if ok {
            self.palette.success(status_text)
        } else {
            self.palette.error(status_text)
        };
        kv.add("status", status_value.into_owned());
        // A dry run executed nothing; say so explicitly so a glance at the summary
        // never reads like a real run in which every unit was a cache hit.
        let header = if summary.dry_run {
            "summary (dry run — no tasks executed)"
        } else {
            "summary"
        };
        write!(self.writer, "{header}\n{kv}").map_err(AppError::internal)?;
        // Flush the final summary so a piped/redirected consumer receives it promptly
        // and it is not lost in a buffer on an abrupt exit.
        self.writer.flush().map_err(AppError::internal)
    }
}

impl HumanReporter<io::Stderr> {
    /// Create a reporter writing human-readable text to process stderr at
    /// `level`.
    ///
    /// Progress, status, and the run summary are human-facing diagnostics, so
    /// they land on stderr; stdout is reserved for the machine-parseable
    /// projection (the Jsonl reporter) and any future structured stdout output.
    #[must_use]
    pub fn stderr(level: Verbosity) -> Self {
        Self::new(io::stderr(), level)
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
            } => {
                // The run-id is log/JSONL correlation noise for an interactive reader, so
                // it is shown only at `-v`; the default line reads `run <intent> on
                // <project>`. The machine JSONL projection always carries the id.
                let line = if matches!(self.level, Verbosity::Verbose) {
                    format!("run {run_id}: {intent} on {project}")
                } else {
                    format!("run {intent} on {project}")
                };
                self.write_line(&line)
            }
            Event::RunFinished { summary } => self.write_summary(summary),
            Event::Warning { message } => {
                let line = format!("warning: {message}");
                self.write_line(&self.palette.warn(&line))
            }
            Event::FullActivation { paths } => {
                let line = format!(
                    "full activation: {} (affects all modules)",
                    paths.join(", ")
                );
                self.write_line(&self.palette.warn(&line))
            }
            Event::PhaseStarted { phase } => {
                self.write_line(&format!("  phase {}: started", phase_label(*phase)))
            }
            Event::PhaseFinished { phase } => {
                self.write_line(&format!("  phase {}: done", phase_label(*phase)))
            }
            Event::PlanPrepared { waves, units } => self.write_line(&format!(
                "plan: {} in {}",
                plural(*units, "unit"),
                plural(*waves, "wave")
            )),
            Event::CacheDecided { unit_id, verdict } => {
                self.write_line(&format!("  cache {unit_id}: {}", verdict_label(*verdict)))
            }
            Event::UnitStarted { unit_id } => self.write_line(&format!("  start {unit_id}")),
            Event::UnitReady { unit_id } => self.write_line(&format!("  ready {unit_id}")),
            Event::UnitFinished { unit_id, status } => {
                let label = self.paint_status(*status);
                self.write_line(&format!("  {label} {unit_id}"))
            }
            Event::WatchStarted { debounce_ms } => self.write_line(&format!(
                "watch: waiting for changes ({debounce_ms}ms debounce)"
            )),
            Event::WatchTriggered { paths } => self.write_line(&format!(
                "watch: {} change(s) triggered a rerun",
                paths.len()
            )),
            Event::WatchRescan => {
                self.write_line("watch: dropped events — re-evaluating the whole workspace")
            }
            Event::WatchStopped => self.write_line("watch: stopped"),
        }
    }
}

/// Render `count` with a naive-pluralized noun: `1 unit`, `2 units`.
///
/// The nouns used here (`unit`, `wave`) pluralize by appending `s`, so a simple
/// suffix rule is correct and keeps the human summary grammatical.
fn plural(count: usize, singular: &str) -> String {
    if count == 1 {
        format!("{count} {singular}")
    } else {
        format!("{count} {singular}s")
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
        UnitStatus::TimedOut => "timed-out",
    }
}

#[cfg(test)]
mod tests {
    use toven_model::{CacheVerdict, Event, Phase, RunStats, UnitStatus};
    use toven_ports::Reporter;

    use super::{HumanReporter, Palette};
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
    fn warning_renders_at_every_verbosity_so_skips_are_never_silent() {
        let warning = Event::Warning {
            message: "ecosystem 'go' skipped: no driver installed".into(),
        };
        for level in [Verbosity::Quiet, Verbosity::Normal, Verbosity::Verbose] {
            let output = render_at(level, std::slice::from_ref(&warning));
            assert_eq!(
                output, "warning: ecosystem 'go' skipped: no driver installed\n",
                "warning must render at {level:?}"
            );
        }
    }

    #[test]
    fn explain_projection_renders_plan_only_stream() {
        // `--explain` / dry-run emits the PLAN-side events + RunFinished, never
        // entering APPLY: no UnitStarted/UnitFinished. Asserted as an exact golden so a
        // layout/ordering regression in the summary block fails.
        let mut summary = RunStats::new(2);
        summary.dry_run = true;
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
plan: 2 units in 1 wave
  cache rust:errors#test: hit
  cache rust:core#test: miss
summary (dry run — no tasks executed)
           planned:  2
               ran:  0
            cached:  1
            failed:  0
           blocked:  0
         cancelled:  0
  failed-readiness:  0
         timed-out:  0
            status:  ok
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

        let expected = "  start u1\n  ok u1\nsummary\n           planned:  1\n               ran:  1\n            cached:  0\n            failed:  0\n           blocked:  0\n         cancelled:  0\n  failed-readiness:  0\n         timed-out:  0\n            status:  ok\n";
        assert_eq!(output, expected);
    }

    #[test]
    fn failed_run_summary_derives_non_zero_exit_from_counters() {
        // The summary's `status` line is derived from the counters by the single owner,
        // never trusted from the event — a failure forces the `failed` status.
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
         timed-out:  0
            status:  failed
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
    fn default_summary_collapses_zero_failure_counters() {
        // A clean run at default verbosity keeps the core counters but drops the
        // all-zero failure family, so the common case reads at a glance.
        let output = render_at(
            Verbosity::Normal,
            &[Event::RunFinished {
                summary: RunStats::new(2),
            }],
        );
        let expected = "\
summary
  planned:  2
      ran:  0
   cached:  0
   status:  ok
";
        assert_eq!(output, expected);
    }

    #[test]
    fn default_summary_keeps_the_non_zero_failure_counters() {
        // Only the failure counters that actually fired are surfaced; the rest of the
        // family still collapses.
        let mut summary = RunStats::new(3);
        summary.failed_units = 1;
        summary.blocked_units = 2;
        let output = render_at(Verbosity::Normal, &[Event::RunFinished { summary }]);
        let expected = "\
summary
  planned:  3
      ran:  0
   cached:  0
   failed:  1
  blocked:  2
   status:  failed
";
        assert_eq!(output, expected);
    }

    #[test]
    fn verbose_summary_keeps_the_full_fixed_table() {
        // `-v` restores every counter (even the zero ones) for a deterministic,
        // fixed-width dump — asserted as the golden the collapse must not touch.
        let output = render_at(
            Verbosity::Verbose,
            &[Event::RunFinished {
                summary: RunStats::new(1),
            }],
        );
        let expected = "\
summary
           planned:  1
               ran:  0
            cached:  0
            failed:  0
           blocked:  0
         cancelled:  0
  failed-readiness:  0
         timed-out:  0
            status:  ok
";
        assert_eq!(output, expected);
    }

    #[test]
    fn every_unit_status_renders_its_label() {
        // Locks the label for each variant so a wrong/duplicated mapping fails; pairs
        // with the compile-time exhaustiveness of `status_label`.
        let cases = [
            (UnitStatus::Cached, "  cached u\n"),
            (UnitStatus::Succeeded, "  ok u\n"),
            (UnitStatus::Failed, "  failed u\n"),
            (UnitStatus::Blocked, "  blocked u\n"),
            (UnitStatus::Cancelled, "  cancelled u\n"),
            (UnitStatus::Ready, "  ready u\n"),
            (UnitStatus::TornDown, "  torn-down u\n"),
            (UnitStatus::FailedReadiness, "  failed-readiness u\n"),
            (UnitStatus::TimedOut, "  timed-out u\n"),
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
    fn palette_colorizes_status_labels_by_outcome_semantics() {
        // An enabled palette wraps the terminal status label in the matching SGR code:
        // green success, red failure, yellow blocked, dim cache hit.
        let cases = [
            (UnitStatus::Succeeded, "\u{1b}[32mok\u{1b}[0m"),
            (UnitStatus::Failed, "\u{1b}[31mfailed\u{1b}[0m"),
            (UnitStatus::Blocked, "\u{1b}[33mblocked\u{1b}[0m"),
            (UnitStatus::Cached, "\u{1b}[2mcached\u{1b}[0m"),
        ];
        for (status, painted) in cases {
            let mut reporter =
                HumanReporter::new(Vec::new(), Verbosity::Verbose).with_palette(Palette::new(true));
            reporter
                .emit(&Event::UnitFinished {
                    unit_id: "u".into(),
                    status,
                })
                .expect("emit");
            let output = String::from_utf8(reporter.into_inner()).expect("utf8");
            assert_eq!(output, format!("  {painted} u\n"), "status {status:?}");
        }
    }

    #[test]
    fn disabled_palette_leaves_status_labels_verbatim() {
        // The default reporter (no palette) and an explicitly disabled palette both
        // render plain text, so a piped or `--color never` run is byte-stable.
        for palette in [None, Some(Palette::new(false))] {
            let mut reporter = HumanReporter::new(Vec::new(), Verbosity::Verbose);
            if let Some(palette) = palette {
                reporter = reporter.with_palette(palette);
            }
            reporter
                .emit(&Event::UnitFinished {
                    unit_id: "u".into(),
                    status: UnitStatus::Succeeded,
                })
                .expect("emit");
            let output = String::from_utf8(reporter.into_inner()).expect("utf8");
            assert_eq!(output, "  ok u\n");
            assert!(!output.contains('\u{1b}'), "no ANSI: {output:?}");
        }
    }

    #[test]
    fn palette_colorizes_the_summary_status_line_by_outcome() {
        // The summary status word is painted green on success (`ok`) and red on failure
        // (`failed`); a disabled palette (covered above) leaves it verbatim for
        // byte-stability.
        let cases = [
            (0, "\u{1b}[32mok\u{1b}[0m"),
            (1, "\u{1b}[31mfailed\u{1b}[0m"),
        ];
        for (failed, painted) in cases {
            let mut summary = RunStats::new(1);
            summary.failed_units = failed;
            let mut reporter =
                HumanReporter::new(Vec::new(), Verbosity::Verbose).with_palette(Palette::new(true));
            reporter
                .emit(&Event::RunFinished { summary })
                .expect("emit");
            let output = String::from_utf8(reporter.into_inner()).expect("utf8");
            assert!(
                output.contains(&format!("status:  {painted}\n")),
                "failed={failed} status not colorized: {output:?}"
            );
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
        // Run start + summary survive; everything in between is suppressed. The run-id
        // is verbose-only, so the non-verbose line omits it.
        assert!(output.starts_with("run build on toven\n"), "{output}");
        assert!(output.contains("summary\n"), "{output}");
        for noise in ["phase ", "plan:", "cache ", "  start ", "  ok "] {
            assert!(!output.contains(noise), "quiet leaked {noise:?}: {output}");
        }
    }

    #[test]
    fn normal_shows_plan_and_terminal_results_but_not_intermediate_noise() {
        let output = render_at(Verbosity::Normal, &full_stream());
        assert!(output.contains("run build on toven\n"), "{output}");
        assert!(output.contains("plan: 1 unit in 1 wave\n"), "{output}");
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
            "plan: 1 unit in 1 wave\n",
            "  cache rust:core#build: miss\n",
            "  start rust:core#build\n",
            "  ok rust:core#build\n",
            "summary\n",
        ] {
            assert!(output.contains(line), "verbose missing {line:?}: {output}");
        }
    }
}
