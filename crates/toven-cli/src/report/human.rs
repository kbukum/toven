//! [`HumanReporter`] — the terminal-facing Event-stream sink.

use std::io::{self, Write};

use rskit_cli::{OutputKV, Palette, Theme, Tone};
use rskit_errors::{AppError, AppResult};
use toven_model::{
    CacheVerdict, CoverageMeasurement, CoverageMetric, CoverageVerdict, Event, Phase, RunStats,
    ToolStatus, UnitStatus,
};
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
    theme: Theme,
    terminal: bool,
    pending_release: Option<String>,
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
            theme: Theme::new(Palette::new(false)),
            terminal: false,
            pending_release: None,
        }
    }

    /// Attach a resolved [`Palette`] so status labels and the summary status line
    /// are colorized; a disabled palette leaves the output verbatim.
    #[must_use]
    pub const fn with_palette(mut self, palette: Palette) -> Self {
        self.theme = Theme::new(palette);
        self
    }

    /// Enable terminal-only in-place progress replacement.
    #[must_use]
    pub const fn with_terminal(mut self, terminal: bool) -> Self {
        self.terminal = terminal;
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
            | Event::ToolAudited { .. }
            | Event::DoctorFinished { .. }
            | Event::WatchStarted { .. }
            | Event::WatchTriggered { .. }
            | Event::WatchRescan
            | Event::WatchStopped => true,
            Event::PlanPrepared { .. } | Event::UnitFinished { .. } => {
                !matches!(level, Verbosity::Quiet)
            }
            // The per-module release/coverage narration is the primary output of
            // its verb (and the whole of a `--dry-run`), so it renders like a
            // terminal unit result — shown at Normal, collapsed only at Quiet.
            // The examining progress line is in-flight status at the same class:
            // it fills the silent decision gap at Normal, suppressed at Quiet.
            Event::ModuleReleaseExamining { .. }
            | Event::ModuleReleaseResolved { .. }
            | Event::ModuleReleaseStaged { .. }
            | Event::ModuleCoverageFinished { .. } => !matches!(level, Verbosity::Quiet),
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
        // Flush each progress line so redirected/piped stdout (block-buffered) surfaces
        // progress promptly instead of in deferred bursts.
        self.writer.flush().map_err(AppError::internal)
    }

    fn labeled_line(
        &self,
        plain_prefix: &str,
        terminal_label: &str,
        detail: &str,
        tone: Tone,
    ) -> String {
        if !self.theme.palette().enabled() {
            return format!("{plain_prefix}{detail}");
        }
        self.theme.action(terminal_label, detail, tone)
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
            self.theme.palette().success(status_text)
        } else {
            self.theme.palette().error(status_text)
        };
        kv.add("status", status_value.into_owned());
        // A dry run executed nothing; say so explicitly so a glance at the summary
        // never reads like a real run in which every unit was a cache hit.
        let header = if summary.dry_run {
            "summary (dry run — no tasks executed)"
        } else {
            "summary"
        };
        let header = self.theme.heading(header);
        write!(self.writer, "{header}\n{kv}").map_err(AppError::internal)?;
        // Flush the final summary so a piped/redirected consumer receives it promptly
        // and it is not lost in a buffer on an abrupt exit.
        self.writer.flush().map_err(AppError::internal)
    }

    /// Render the run-start line. The run-id is log/JSONL correlation noise for
    /// an interactive reader, so it is shown only at `-v`; the default line reads
    /// `run <intent> on <project>`. The machine JSONL projection always carries
    /// the id.
    fn write_run_started(&mut self, run_id: &str, intent: &str, project: &str) -> AppResult<()> {
        let detail = if matches!(self.level, Verbosity::Verbose) {
            format!("{run_id}: {intent} on {project}")
        } else {
            format!("{intent} on {project}")
        };
        let line = self.labeled_line("run ", "Running", &detail, Tone::Info);
        self.write_line(&line)
    }

    /// Render a per-module release *examining* progress line, before its slow
    /// decision I/O (baseline resolution, change detection, registry lookup).
    ///
    /// Uncolored in-flight status — not a verdict — so it reads as `checking
    /// X…`, filling the otherwise-silent gap before that module's settled
    /// `release X: …` decision line.
    ///
    /// Verbosity tradeoff: this renders at Normal (the whole point is to fill
    /// the gap an operator watches). If it proves noisy for very large
    /// workspaces the fallback is to gate it to Verbose in [`Self::renders`].
    fn write_release_examining(&mut self, module: &str) -> AppResult<()> {
        let line = self.labeled_line("  checking ", "Checking", &format!("{module}…"), Tone::Info);
        self.write_line(&line)?;
        self.pending_release = self.terminal.then(|| module.to_string());
        Ok(())
    }

    /// Render a per-module release *decision* line (before any mutation).
    ///
    /// A planned transition a reader sees take shape per module; every decision
    /// reads honestly rather than as a bogus version change. The four shapes:
    /// an already-released module is `already at X`; a genuine own-version bump
    /// is `X → Y (level)`; a first cut at the declared version (no numeric move
    /// yet a real release) is `initial release X` rather than a no-op `X → X`;
    /// and a dependency-floor-only entry (no own-version bump) is
    /// `X (dependency floor)`.
    fn write_release_resolved(
        &mut self,
        module: &str,
        current_version: &str,
        planned_version: Option<&str>,
        level: &str,
        reason: &str,
        up_to_date: bool,
    ) -> AppResult<()> {
        let (detail, label, tone) = if up_to_date {
            (
                format!("{module}: already at {current_version}"),
                "Unchanged",
                Tone::Dim,
            )
        } else if reason == "no-change" {
            (
                format!("{module}: no change ({current_version})"),
                "Unchanged",
                Tone::Dim,
            )
        } else if let Some(planned) = planned_version {
            if planned == current_version {
                match reason {
                    "initial-release" => (
                        format!("{module}: initial release {planned}"),
                        "Releasing",
                        Tone::Success,
                    ),
                    other => (
                        format!("{module}: release {planned} ({other})"),
                        "Releasing",
                        Tone::Success,
                    ),
                }
            } else {
                (
                    format!("{module}: {current_version} → {planned} ({level})"),
                    "Releasing",
                    Tone::Success,
                )
            }
        } else {
            (
                format!("{module}: {current_version} (dependency floor)"),
                "Updating",
                Tone::Warning,
            )
        };
        let line = self.labeled_line("  release ", label, &detail, tone);
        let replace = self.terminal && self.pending_release.as_deref() == Some(module);
        self.pending_release = None;
        if replace {
            writeln!(self.writer, "\u{1b}[1A\r\u{1b}[2K{line}").map_err(AppError::internal)?;
            self.writer.flush().map_err(AppError::internal)
        } else {
            self.write_line(&line)
        }
    }

    /// Render a per-module release *commit* line (after the side effect landed).
    ///
    /// Confirms the decision only once the module's mutation is real; the
    /// created tag, when any, rides the same line.
    fn write_release_staged(
        &mut self,
        module: &str,
        new_version: &str,
        tag: Option<&str>,
    ) -> AppResult<()> {
        let detail = tag.map_or_else(
            || format!("{module}: staged {new_version}"),
            |tag| format!("{module}: staged {new_version} (tag {tag})"),
        );
        let line = self.labeled_line("  release ", "Staged", &detail, Tone::Success);
        self.write_line(&line)
    }

    /// Render a per-module coverage verdict as one line, colorized by outcome.
    fn write_coverage(
        &mut self,
        module: &str,
        measurements: &[CoverageMeasurement],
        verdict: CoverageVerdict,
    ) -> AppResult<()> {
        let detail = format_measurements(measurements);
        let label = coverage_verdict_label(verdict);
        let detail = if detail.is_empty() {
            format!("{module}: {label}")
        } else {
            format!("{module}: {label} ({detail})")
        };
        let (terminal_label, tone) = match verdict {
            CoverageVerdict::Passed => ("Passed", Tone::Success),
            CoverageVerdict::Failed => ("Failed", Tone::Error),
            CoverageVerdict::Advisory | CoverageVerdict::Excluded => ("Advisory", Tone::Warning),
        };
        let line = self.labeled_line("  coverage ", terminal_label, &detail, tone);
        self.write_line(&line)
    }

    /// Render a per-tool `doctor` audit line, colorized by presence.
    fn write_tool_audited(
        &mut self,
        label: &str,
        program: &str,
        status: &ToolStatus,
    ) -> AppResult<()> {
        match status {
            ToolStatus::Present { version } => {
                let detail = version.as_ref().map_or_else(
                    || format!("{label} ({program}): present"),
                    |version| format!("{label} ({program}): present ({version})"),
                );
                let line = self.labeled_line("  tool ", "Found", &detail, Tone::Success);
                self.write_line(&line)
            }
            ToolStatus::Missing => {
                let detail = format!("{label} ({program}): missing");
                let line = self.labeled_line("  tool ", "Missing", &detail, Tone::Error);
                self.write_line(&line)
            }
        }
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
    #[allow(clippy::too_many_lines)] // a flat event→line dispatch table: one arm per Event variant
    fn emit(&mut self, event: &Event) -> AppResult<()> {
        if !Self::renders(self.level, event) {
            return Ok(());
        }
        match event {
            Event::RunStarted {
                run_id,
                intent,
                project,
            } => self.write_run_started(run_id, intent, project),
            Event::RunFinished { summary } => self.write_summary(summary),
            Event::Warning { message } => {
                let line = self.labeled_line("warning: ", "Warning", message, Tone::Warning);
                self.write_line(&line)
            }
            Event::FullActivation { paths } => {
                let detail = format!("{} (affects all modules)", paths.join(", "));
                let line =
                    self.labeled_line("full activation: ", "Activating", &detail, Tone::Warning);
                self.write_line(&line)
            }
            Event::PhaseStarted { phase } => {
                let detail = format!("{}: started", phase_label(*phase));
                let line = self.labeled_line("  phase ", "Starting", &detail, Tone::Info);
                self.write_line(&line)
            }
            Event::PhaseFinished { phase } => {
                let detail = format!("{}: done", phase_label(*phase));
                let line = self.labeled_line("  phase ", "Finished", &detail, Tone::Success);
                self.write_line(&line)
            }
            Event::PlanPrepared { waves, units } => {
                let detail = format!("{} in {}", plural(*units, "unit"), plural(*waves, "wave"));
                let line = self.labeled_line("plan: ", "Planning", &detail, Tone::Info);
                self.write_line(&line)
            }
            Event::CacheDecided { unit_id, verdict } => {
                let detail = format!("{unit_id}: {}", verdict_label(*verdict));
                let tone = if matches!(verdict, CacheVerdict::Hit) {
                    Tone::Dim
                } else {
                    Tone::Info
                };
                let line = self.labeled_line("  cache ", "Cache", &detail, tone);
                self.write_line(&line)
            }
            Event::UnitStarted { unit_id } => {
                let line = self.labeled_line("  start ", "Running", unit_id, Tone::Info);
                self.write_line(&line)
            }
            Event::UnitReady { unit_id } => {
                let line = self.labeled_line("  ready ", "Ready", unit_id, Tone::Success);
                self.write_line(&line)
            }
            Event::UnitFinished {
                unit_id,
                status,
                exit_code,
            } => {
                // Name the non-zero exit on the failure line so a reader sees *why* the
                // unit failed without correlating against the separate output stream; the
                // captured stdout/stderr already surfaced there.
                let detail = exit_code.as_ref().map_or_else(
                    || unit_id.clone(),
                    |code| format!("{unit_id} (exit {code})"),
                );
                let tone = match status {
                    UnitStatus::Succeeded | UnitStatus::Ready | UnitStatus::TornDown => {
                        Tone::Success
                    }
                    UnitStatus::Failed | UnitStatus::FailedReadiness | UnitStatus::TimedOut => {
                        Tone::Error
                    }
                    UnitStatus::Blocked | UnitStatus::Cancelled => Tone::Warning,
                    UnitStatus::Cached => Tone::Dim,
                };
                let prefix = format!("  {} ", status_label(*status));
                let line = self.labeled_line(&prefix, status_label(*status), &detail, tone);
                self.write_line(&line)
            }
            Event::ModuleReleaseExamining { module } => self.write_release_examining(module),
            Event::ModuleReleaseResolved {
                module,
                current_version,
                planned_version,
                level,
                reason,
                up_to_date,
                ..
            } => self.write_release_resolved(
                module,
                current_version,
                planned_version.as_deref(),
                level,
                reason,
                *up_to_date,
            ),
            Event::ModuleReleaseStaged {
                module,
                new_version,
                tag,
                ..
            } => self.write_release_staged(module, new_version, tag.as_deref()),
            Event::ModuleCoverageFinished {
                module,
                measurements,
                verdict,
            } => self.write_coverage(module, measurements, *verdict),
            Event::WatchStarted { debounce_ms } => {
                let detail = format!("waiting for changes ({debounce_ms}ms debounce)");
                let line = self.labeled_line("watch: ", "Watching", &detail, Tone::Info);
                self.write_line(&line)
            }
            Event::WatchTriggered { paths } => {
                let detail = format!("{} change(s) triggered a rerun", paths.len());
                let line = self.labeled_line("watch: ", "Changed", &detail, Tone::Info);
                self.write_line(&line)
            }
            Event::WatchRescan => {
                let line = self.labeled_line(
                    "watch: ",
                    "Rescanning",
                    "dropped events — re-evaluating the whole workspace",
                    Tone::Warning,
                );
                self.write_line(&line)
            }
            Event::WatchStopped => {
                let line = self.labeled_line("watch: ", "Stopped", "stopped", Tone::Dim);
                self.write_line(&line)
            }
            Event::ToolAudited {
                label,
                program,
                status,
            } => self.write_tool_audited(label, program, status),
            Event::DoctorFinished { checked, missing } => {
                let detail = format!("{checked} checked, {missing} missing");
                let (label, tone) = if *missing == 0 {
                    ("Healthy", Tone::Success)
                } else {
                    ("Incomplete", Tone::Error)
                };
                let line = self.labeled_line("doctor: ", label, &detail, tone);
                self.write_line(&line)
            }
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

const fn coverage_verdict_label(verdict: CoverageVerdict) -> &'static str {
    match verdict {
        CoverageVerdict::Passed => "passed",
        CoverageVerdict::Failed => "failed",
        CoverageVerdict::Advisory => "advisory",
        CoverageVerdict::Excluded => "excluded",
    }
}

const fn metric_label(metric: CoverageMetric) -> &'static str {
    match metric {
        CoverageMetric::Line => "line",
        CoverageMetric::Function => "function",
        CoverageMetric::Region => "region",
        CoverageMetric::ChangedLine => "changed-line",
    }
}

/// Render a basis-point percentage (`9537`) as `95.37%`.
fn percent(basis_points: u32) -> String {
    format!("{}.{:02}%", basis_points / 100, basis_points % 100)
}

/// Render the per-dimension measurements as one compact, comma-separated detail
/// string: `line 95.37%, function 90.00% (<95.00%)`. A dimension below its floor
/// is annotated with the floor it missed so the verdict is self-explaining.
fn format_measurements(measurements: &[CoverageMeasurement]) -> String {
    measurements
        .iter()
        .map(|m| {
            let head = format!("{} {}", metric_label(m.metric), percent(m.measured));
            match m.threshold {
                Some(threshold) if !m.met => format!("{head} (<{})", percent(threshold)),
                _ => head,
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use toven_model::{
        CacheVerdict, CoverageMeasurement, CoverageMetric, CoverageVerdict, Event, Phase, RunStats,
        UnitStatus,
    };
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
                exit_code: None,
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
                exit_code: None,
            }]);
            assert_eq!(output, expected, "status {status:?}");
        }
    }

    #[test]
    fn a_failed_unit_names_its_non_zero_exit_code() {
        // The exit code annotates the failure line so a reader sees *why* a unit failed
        // without correlating against the separate raw-output stream; a success (`None`)
        // stays unannotated.
        let output = render(&[Event::UnitFinished {
            unit_id: "command:boom#check".into(),
            status: UnitStatus::Failed,
            exit_code: Some(3),
        }]);
        assert_eq!(output, "  failed command:boom#check (exit 3)\n");
    }

    #[test]
    fn palette_colorizes_status_labels_by_outcome_semantics() {
        // Cargo-like terminal labels are right-aligned, bold, and colored by
        // outcome while the detail remains unstyled.
        let cases = [
            (
                UnitStatus::Succeeded,
                "\u{1b}[1m\u{1b}[32m          ok\u{1b}[0m\u{1b}[0m u\n",
            ),
            (
                UnitStatus::Failed,
                "\u{1b}[1m\u{1b}[31m      failed\u{1b}[0m\u{1b}[0m u\n",
            ),
            (
                UnitStatus::Blocked,
                "\u{1b}[1m\u{1b}[33m     blocked\u{1b}[0m\u{1b}[0m u\n",
            ),
            (
                UnitStatus::Cached,
                "\u{1b}[1m\u{1b}[2m      cached\u{1b}[0m\u{1b}[0m u\n",
            ),
        ];
        for (status, expected) in cases {
            let mut reporter =
                HumanReporter::new(Vec::new(), Verbosity::Verbose).with_palette(Palette::new(true));
            reporter
                .emit(&Event::UnitFinished {
                    unit_id: "u".into(),
                    status,
                    exit_code: None,
                })
                .expect("emit");
            let output = String::from_utf8(reporter.into_inner()).expect("utf8");
            assert_eq!(output, expected, "status {status:?}");
        }
    }

    #[test]
    fn palette_styles_release_progress_without_coloring_its_detail() {
        let mut reporter =
            HumanReporter::new(Vec::new(), Verbosity::Normal).with_palette(Palette::new(true));
        reporter
            .emit(&Event::ModuleReleaseExamining {
                module: "rust:core".into(),
            })
            .expect("examining");
        reporter
            .emit(&Event::ModuleReleaseResolved {
                module: "rust:core".into(),
                current_version: "1.2.0".into(),
                planned_version: Some("1.3.0".into()),
                level: "minor".into(),
                reason: "changed".into(),
                tag: None,
                publication: None,
                up_to_date: false,
            })
            .expect("resolved");
        assert_eq!(
            String::from_utf8(reporter.into_inner()).expect("utf8"),
            concat!(
                "\u{1b}[1m\u{1b}[36m    Checking\u{1b}[0m\u{1b}[0m rust:core…\n",
                "\u{1b}[1m\u{1b}[32m   Releasing\u{1b}[0m\u{1b}[0m rust:core: 1.2.0 → 1.3.0 (minor)\n",
            )
        );
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
                    exit_code: None,
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
                exit_code: None,
            },
        ];
        let output = render(&events);
        assert_eq!(output, "  ready srv\n  torn-down srv\n");
    }

    #[test]
    fn an_examining_progress_line_precedes_the_modules_resolved_decision() {
        // The `checking <module>…` progress signal fills the otherwise-silent
        // gap during the slow decision I/O, then the settled decision follows.
        // Asserted as an exact golden so the progress → decision rhythm stays
        // organized.
        let events = vec![
            Event::ModuleReleaseExamining {
                module: "core".into(),
            },
            Event::ModuleReleaseResolved {
                module: "core".into(),
                current_version: "1.2.0".into(),
                planned_version: Some("1.3.0".into()),
                level: "minor".into(),
                reason: "changed".into(),
                tag: Some("core-v1.3.0".into()),
                publication: Some("publish".into()),
                up_to_date: false,
            },
        ];
        assert_eq!(
            render(&events),
            "  checking core…\n  release core: 1.2.0 → 1.3.0 (minor)\n"
        );
    }

    #[test]
    fn a_terminal_replaces_the_examining_line_with_the_resolved_decision() {
        let events = [
            Event::ModuleReleaseExamining {
                module: "core".into(),
            },
            Event::ModuleReleaseResolved {
                module: "core".into(),
                current_version: "1.2.0".into(),
                planned_version: Some("1.3.0".into()),
                level: "minor".into(),
                reason: "changed".into(),
                tag: Some("core-v1.3.0".into()),
                publication: Some("publish".into()),
                up_to_date: false,
            },
        ];
        let mut reporter = HumanReporter::new(Vec::new(), Verbosity::Normal).with_terminal(true);
        for event in &events {
            reporter.emit(event).expect("emit");
        }
        assert_eq!(
            String::from_utf8(reporter.into_inner()).expect("utf8"),
            "  checking core…\n\u{1b}[1A\r\u{1b}[2K  release core: 1.2.0 → 1.3.0 (minor)\n"
        );
    }

    #[test]
    fn an_examining_progress_line_is_collapsed_at_quiet() {
        // Progress is narration, not a verdict: like the resolved decision it
        // renders at Normal but is suppressed at Quiet.
        let examining = Event::ModuleReleaseExamining {
            module: "core".into(),
        };
        assert_eq!(
            render_at(Verbosity::Normal, std::slice::from_ref(&examining)),
            "  checking core…\n"
        );
        assert_eq!(
            render_at(Verbosity::Quiet, std::slice::from_ref(&examining)),
            ""
        );
    }

    #[test]
    fn release_decision_then_commit_reads_as_one_narration() {
        // A resolved decision narrates the planned transition; the later staged
        // event confirms the same module as the transaction lands. Asserted as an
        // exact golden so the decision → commit rhythm stays organized.
        let events = vec![
            Event::ModuleReleaseResolved {
                module: "core".into(),
                current_version: "1.2.0".into(),
                planned_version: Some("1.3.0".into()),
                level: "minor".into(),
                reason: "changed".into(),
                tag: Some("core-v1.3.0".into()),
                publication: Some("publish".into()),
                up_to_date: false,
            },
            Event::ModuleReleaseStaged {
                module: "core".into(),
                new_version: "1.3.0".into(),
                manifests: vec!["crates/core/Cargo.toml".into()],
                changelog: Some("crates/core/CHANGELOG.md".into()),
                tag: Some("core-v1.3.0".into()),
            },
        ];
        assert_eq!(
            render(&events),
            "  release core: 1.2.0 → 1.3.0 (minor)\n  release core: staged 1.3.0 (tag core-v1.3.0)\n"
        );
    }

    #[test]
    fn an_up_to_date_or_floor_only_decision_reads_honestly() {
        // No planned bump must never render as a bogus version change.
        let up_to_date = Event::ModuleReleaseResolved {
            module: "leaf".into(),
            current_version: "0.4.1".into(),
            planned_version: None,
            level: "patch".into(),
            reason: "changed".into(),
            tag: None,
            publication: None,
            up_to_date: true,
        };
        assert_eq!(
            render(std::slice::from_ref(&up_to_date)),
            "  release leaf: already at 0.4.1\n"
        );

        let floor_only = Event::ModuleReleaseResolved {
            module: "leaf".into(),
            current_version: "0.4.1".into(),
            planned_version: None,
            level: "patch".into(),
            reason: "dependency-cascade".into(),
            tag: None,
            publication: None,
            up_to_date: false,
        };
        assert_eq!(
            render(std::slice::from_ref(&floor_only)),
            "  release leaf: 0.4.1 (dependency floor)\n"
        );

        let no_change = Event::ModuleReleaseResolved {
            module: "idle".into(),
            current_version: "0.4.1".into(),
            planned_version: None,
            level: "patch".into(),
            reason: "no-change".into(),
            tag: None,
            publication: None,
            up_to_date: false,
        };
        assert_eq!(
            render(std::slice::from_ref(&no_change)),
            "  release idle: no change (0.4.1)\n"
        );
    }

    #[test]
    fn an_initial_release_reads_as_a_first_cut_not_a_no_op() {
        // A first release cuts the version the module already declares, so
        // current == planned. It must read as a real release, never a bogus
        // `0.1.0 → 0.1.0` transition.
        let initial = Event::ModuleReleaseResolved {
            module: "core".into(),
            current_version: "0.1.0".into(),
            planned_version: Some("0.1.0".into()),
            level: "minor".into(),
            reason: "initial-release".into(),
            tag: Some("core-v0.1.0".into()),
            publication: Some("registry".into()),
            up_to_date: false,
        };
        assert_eq!(
            render(std::slice::from_ref(&initial)),
            "  release core: initial release 0.1.0\n"
        );
    }

    #[test]
    fn a_tag_less_stage_omits_the_tag_suffix() {
        let staged = Event::ModuleReleaseStaged {
            module: "leaf".into(),
            new_version: "0.4.2".into(),
            manifests: Vec::new(),
            changelog: None,
            tag: None,
        };
        assert_eq!(
            render(std::slice::from_ref(&staged)),
            "  release leaf: staged 0.4.2\n"
        );
    }

    #[test]
    fn coverage_verdict_is_one_line_per_module_with_measurements() {
        let passing = Event::ModuleCoverageFinished {
            module: "core".into(),
            measurements: vec![CoverageMeasurement {
                metric: CoverageMetric::Line,
                measured: 9537,
                threshold: Some(9000),
                met: true,
            }],
            verdict: CoverageVerdict::Passed,
        };
        assert_eq!(
            render(std::slice::from_ref(&passing)),
            "  coverage core: passed (line 95.37%)\n"
        );

        // A below-floor dimension annotates the floor it missed on the fail line.
        let failing = Event::ModuleCoverageFinished {
            module: "leaf".into(),
            measurements: vec![CoverageMeasurement {
                metric: CoverageMetric::Function,
                measured: 8000,
                threshold: Some(9000),
                met: false,
            }],
            verdict: CoverageVerdict::Failed,
        };
        assert_eq!(
            render(std::slice::from_ref(&failing)),
            "  coverage leaf: failed (function 80.00% (<90.00%))\n"
        );
    }

    #[test]
    fn progressive_release_and_coverage_lines_collapse_at_quiet() {
        // The per-module narration is Normal-level output, collapsed only at Quiet
        // (like a terminal unit result), so a Quiet run leaves just the summary.
        let events = [
            Event::ModuleReleaseResolved {
                module: "core".into(),
                current_version: "1.2.0".into(),
                planned_version: Some("1.3.0".into()),
                level: "minor".into(),
                reason: "changed".into(),
                tag: None,
                publication: None,
                up_to_date: false,
            },
            Event::ModuleCoverageFinished {
                module: "core".into(),
                measurements: Vec::new(),
                verdict: CoverageVerdict::Passed,
            },
        ];
        assert_eq!(render_at(Verbosity::Quiet, &events), "");
        assert_eq!(
            render_at(Verbosity::Normal, &events),
            "  release core: 1.2.0 → 1.3.0 (minor)\n  coverage core: passed\n"
        );
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
                exit_code: None,
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
