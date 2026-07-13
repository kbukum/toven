//! [`TilesRawSink`] — the live multi-region terminal adapter for the engine's
//! raw-output channel.
//!
//! Where [`WriterRawSink`](super::WriterRawSink) renders one linear stream (the
//! `stream` view), this sink de-interleaves concurrent output *spatially*: each
//! in-flight unit gets its own tile that grows with its output — from a single
//! header line for a silent or instantly-finishing unit up to a bounded tail —
//! so units can all stream live under full parallelism without their bytes
//! intermixing. It reports
//! [`supports_concurrent_live`](toven_ports::RawOutputSink::supports_concurrent_live),
//! so the engine drives every unit through the
//! [`begin_unit`](toven_ports::RawOutputSink::begin_unit) /
//! [`live`](toven_ports::RawOutputSink::live) /
//! [`end_unit`](toven_ports::RawOutputSink::end_unit) lifecycle instead of
//! buffering normal units into blocks. All rendering is delegated to rskit's
//! generic [`LiveConsole`]; this adapter only maps the port calls and owns the
//! status header and verdict styling.

use std::collections::HashMap;

use rskit_cli::{LiveConfig, LiveConsole, Palette};
use rskit_errors::AppResult;
use toven_model::{UnitOutput, UnitStatus};
use toven_ports::RawOutputSink;

use super::summary::{RunSummary, SummaryScanner};

/// Maximum content lines a unit tile grows to before its tail is capped. The
/// tile starts at just its header and grows with the unit's output up to this
/// height (a quiet or instantly-finishing unit stays a single line), so this is
/// a ceiling rather than a reserved block. Also the PTY row count the CLI sizes
/// live units to, so a child's own cursor math matches the tile's grid height.
pub(super) const TILE_TAIL_LINES: u16 = 20;

/// Running lifecycle tallies rendered into the status header.
#[derive(Debug, Default, Clone, Copy)]
struct Counts {
    running: usize,
    done: usize,
    failed: usize,
}

/// Renders the engine's per-unit raw output as a live grid of tiles on stderr.
pub struct TilesRawSink {
    console: LiveConsole,
    palette: Palette,
    counts: Counts,
    summaries: HashMap<String, SummaryScanner>,
    failures: Vec<FailureRecord>,
}

/// A finished failed unit retained for the end-of-run failure epilogue: its
/// colorized verdict line and the replayed failure transcript (un-prefixed, in
/// order), so all failures can be re-surfaced together above the run summary.
struct FailureRecord {
    verdict: String,
    body: Vec<String>,
}

impl TilesRawSink {
    /// Create a tiles sink rendering to stderr, laying each tile out as a
    /// `TILE_TAIL_LINES`-row by `width`-column virtual terminal and colorizing
    /// verdicts per `palette`.
    #[must_use]
    pub fn stderr(width: usize, palette: Palette) -> Self {
        Self::with_console(
            LiveConsole::to_stderr(LiveConfig {
                rows: TILE_TAIL_LINES as usize,
                cols: width,
                ..LiveConfig::default()
            }),
            palette,
        )
    }

    /// Create a tiles sink whose rendering is discarded — for tests and
    /// non-terminal runs where the live area must not draw. The grid still needs
    /// a finite width, so a conventional 80-column terminal is assumed.
    #[must_use]
    pub fn hidden() -> Self {
        Self::with_console(
            LiveConsole::hidden(LiveConfig {
                rows: TILE_TAIL_LINES as usize,
                cols: 80,
                ..LiveConfig::default()
            }),
            Palette::new(false),
        )
    }

    fn with_console(console: LiveConsole, palette: Palette) -> Self {
        Self {
            console,
            palette,
            counts: Counts::default(),
            summaries: HashMap::new(),
            failures: Vec::new(),
        }
    }

    /// The grid width a live child's PTY must be sized to so its own line
    /// wrapping matches the tile: the tile width minus the content indent. A
    /// child told it is `cols` wide would wrap a full-width progress redraw at
    /// the narrower grid edge, scrolling the short grid and leaking a stale
    /// frame to scrollback each tick.
    pub(super) fn content_cols(&self) -> usize {
        self.console.content_cols()
    }

    fn refresh_header(&self) {
        let counts = self.counts;
        self.console.set_header(format!(
            "running {} · done {} · failed {}",
            counts.running, counts.done, counts.failed
        ));
    }
}

impl RawOutputSink for TilesRawSink {
    fn live(&mut self, chunk: &UnitOutput) -> AppResult<()> {
        self.console.feed(&chunk.unit_id, &chunk.bytes);
        if let Some(scanner) = self.summaries.get_mut(&chunk.unit_id) {
            scanner.observe(&chunk.bytes);
        } else {
            self.summaries
                .entry(chunk.unit_id.clone())
                .or_default()
                .observe(&chunk.bytes);
        }
        Ok(())
    }

    fn block(&mut self, unit_id: &str, chunks: &[UnitOutput]) -> AppResult<()> {
        // Concurrent-live sinks stream every unit through the live lifecycle, so
        // this path is only reached defensively (e.g. a spilled buffered block
        // from a unit that was never live-tailed). Flush it to scrollback rather
        // than dropping the output.
        self.console.note(format!("==> {unit_id}"))?;
        for chunk in chunks {
            let text = String::from_utf8_lossy(&chunk.bytes);
            self.console.note(text.trim_end_matches('\n'))?;
        }
        Ok(())
    }

    fn supports_concurrent_live(&self) -> bool {
        true
    }

    fn begin_unit(&mut self, unit_id: &str, label: &str) -> AppResult<()> {
        self.console.begin(unit_id, label);
        self.counts.running += 1;
        self.refresh_header();
        Ok(())
    }

    fn end_unit(&mut self, unit_id: &str, status: UnitStatus) -> AppResult<()> {
        let summary = self.summaries.remove(unit_id).and_then(|s| s.summary());
        let verdict = verdict_line(self.palette, unit_id, status, summary);
        if status.is_failure() {
            // A failure is the one case detail matters: replay the retained tail
            // contiguously under the red verdict, and retain it so `finish_run`
            // can re-surface every failure together above the run summary.
            let body = self.console.finish_with_replay(unit_id, &verdict)?;
            self.failures.push(FailureRecord { verdict, body });
        } else {
            // Success collapses to a single verdict line — no PASS flood.
            self.console.finish(unit_id, verdict)?;
        }
        self.counts.running = self.counts.running.saturating_sub(1);
        if status.is_failure() {
            self.counts.failed += 1;
        } else {
            self.counts.done += 1;
        }
        self.refresh_header();
        Ok(())
    }

    fn finish_run(&mut self) -> AppResult<()> {
        if self.failures.is_empty() {
            return Ok(());
        }
        // Re-surface every failure as one contiguous section once the live area
        // has drained, so failing units are not buried above a flood of later
        // per-unit output — the section lands directly above the run summary.
        self.console.note("")?;
        let heading = format!("failures ({}):", self.failures.len());
        self.console.note(self.palette.error(&heading).as_ref())?;
        for record in &self.failures {
            self.console.note(&record.verdict)?;
            for line in &record.body {
                self.console.note(format!("  {line}"))?;
            }
        }
        Ok(())
    }
}

/// Render a finished unit's one-line verdict: a colorized outcome label, the id,
/// and — for a succeeding unit with a parsed count summary — a `· N passed`
/// tail folding the runner's own totals.
///
/// Colors match the human reporter: green success, red failure, yellow
/// blocked/cancelled, dim cache hit.
pub(crate) fn verdict_line(
    palette: Palette,
    unit_id: &str,
    status: UnitStatus,
    summary: Option<RunSummary>,
) -> String {
    let label = status_label(status);
    let painted = match status {
        UnitStatus::Succeeded | UnitStatus::Ready | UnitStatus::TornDown => palette.success(label),
        UnitStatus::Failed | UnitStatus::FailedReadiness | UnitStatus::TimedOut => {
            palette.error(label)
        }
        UnitStatus::Blocked | UnitStatus::Cancelled => palette.warn(label),
        UnitStatus::Cached => palette.dim(label),
    };
    summary.filter(|_| !status.is_failure()).map_or_else(
        || format!("{painted} {unit_id}"),
        |summary| format!("{painted} {unit_id} · {summary}"),
    )
}

/// The short outcome word for a status, matching the human reporter's labels.
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
    use rskit_cli::Palette;
    use toven_model::{OutputStream, UnitOutput, UnitStatus};
    use toven_ports::RawOutputSink;

    use super::{RunSummary, TilesRawSink, status_label, verdict_line};

    fn chunk(unit: &str, bytes: &[u8]) -> UnitOutput {
        UnitOutput {
            unit_id: unit.into(),
            stream: OutputStream::Stdout,
            bytes: bytes.to_vec(),
        }
    }

    #[test]
    fn advertises_concurrent_live_support() {
        let sink = TilesRawSink::hidden();
        assert!(sink.supports_concurrent_live());
    }

    #[test]
    fn content_cols_reserves_the_tile_indent() {
        // Children are sized to this, not the full tile width, so a full-width
        // in-place progress redraw does not wrap into a grid scroll.
        assert_eq!(
            TilesRawSink::stderr(120, Palette::new(false)).content_cols(),
            118
        );
    }

    #[test]
    fn drives_the_full_unit_lifecycle_without_panicking() {
        let mut sink = TilesRawSink::hidden();
        sink.begin_unit("rust:core#test", "rust:core#test").unwrap();
        sink.begin_unit("rust:cli#test", "rust:cli#test").unwrap();
        sink.live(&chunk("rust:core#test", b"compiling\n")).unwrap();
        sink.live(&chunk("rust:cli#test", b"running 3 tests\n"))
            .unwrap();
        sink.end_unit("rust:core#test", UnitStatus::Succeeded)
            .unwrap();
        sink.end_unit("rust:cli#test", UnitStatus::Failed).unwrap();
    }

    #[test]
    fn a_failed_unit_is_retained_for_the_end_of_run_epilogue() {
        // A failure both replays inline (at end_unit) and is retained so
        // finish_run can re-surface it above the summary; a success is not.
        let mut sink = TilesRawSink::hidden();
        sink.begin_unit("go:auth#test", "go:auth#test").unwrap();
        sink.live(&chunk("go:auth#test", b"--- FAIL: TestParse\nFAIL\n"))
            .unwrap();
        sink.end_unit("go:auth#test", UnitStatus::Failed).unwrap();
        sink.begin_unit("go:ok#test", "go:ok#test").unwrap();
        sink.end_unit("go:ok#test", UnitStatus::Succeeded).unwrap();

        assert_eq!(sink.failures.len(), 1);
        assert!(sink.failures[0].verdict.contains("go:auth#test"));
        assert_eq!(
            sink.failures[0].body,
            vec!["--- FAIL: TestParse".to_string(), "FAIL".to_string()]
        );
        // The consolidated epilogue renders without panicking (hidden console).
        sink.finish_run().unwrap();
    }

    #[test]
    fn finish_run_is_a_no_op_when_nothing_failed() {
        let mut sink = TilesRawSink::hidden();
        sink.begin_unit("u", "u").unwrap();
        sink.end_unit("u", UnitStatus::Succeeded).unwrap();
        assert!(sink.failures.is_empty());
        sink.finish_run().unwrap();
    }

    #[test]
    fn a_finished_unit_summary_is_consumed_once() {
        // The per-unit summary scanner is dropped at end_unit, so a re-used id
        // does not carry a stale tally.
        let mut sink = TilesRawSink::hidden();
        sink.begin_unit("u", "u").unwrap();
        sink.live(&chunk(
            "u",
            b"Summary [0.1s] 5 tests run: 5 passed, 0 skipped\n",
        ))
        .unwrap();
        sink.end_unit("u", UnitStatus::Succeeded).unwrap();
        assert!(sink.summaries.is_empty());
    }

    #[test]
    fn block_flushes_to_scrollback_without_dropping_output() {
        let mut sink = TilesRawSink::hidden();
        sink.block(
            "normal",
            &[chunk("normal", b"line a\n"), chunk("normal", b"line b\n")],
        )
        .unwrap();
    }

    #[test]
    fn verdict_line_carries_label_and_id() {
        let palette = Palette::new(false);
        assert_eq!(
            verdict_line(palette, "rust:core#test", UnitStatus::Succeeded, None),
            "ok rust:core#test"
        );
        assert_eq!(
            verdict_line(palette, "rust:cli#test", UnitStatus::Failed, None),
            "failed rust:cli#test"
        );
    }

    #[test]
    fn succeeding_verdict_folds_the_count_summary() {
        let palette = Palette::new(false);
        let summary = summary_of(b"Summary [0.1s] 987 tests run: 987 passed, 3 skipped\n");
        assert_eq!(
            verdict_line(palette, "rust:core#test", UnitStatus::Succeeded, summary),
            "ok rust:core#test · 987 passed, 3 skipped"
        );
    }

    #[test]
    fn failing_verdict_drops_the_count_summary() {
        // A failed unit gets a failure replay instead of a collapsed count tail.
        let palette = Palette::new(false);
        let summary = summary_of(b"Summary [0.1s] 5 tests run: 4 passed, 0 skipped\n");
        assert_eq!(
            verdict_line(palette, "rust:cli#test", UnitStatus::Failed, summary),
            "failed rust:cli#test"
        );
    }

    fn summary_of(bytes: &[u8]) -> Option<RunSummary> {
        let mut scanner = super::SummaryScanner::default();
        scanner.observe(bytes);
        scanner.summary()
    }

    #[test]
    fn status_label_covers_every_variant() {
        for status in [
            UnitStatus::Cached,
            UnitStatus::Succeeded,
            UnitStatus::Failed,
            UnitStatus::Blocked,
            UnitStatus::Cancelled,
            UnitStatus::Ready,
            UnitStatus::TornDown,
            UnitStatus::FailedReadiness,
            UnitStatus::TimedOut,
        ] {
            assert!(!status_label(status).is_empty());
        }
    }
}
