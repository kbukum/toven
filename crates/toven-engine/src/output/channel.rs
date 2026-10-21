//! [`UnitOutputChannel`] — buffers normal units, live-tails persistent ones.
//!
//! The channel is the engine-owned policy layer between the APPLY exec loop and
//! a [`RawOutputSink`]. It groups raw output deterministically under
//! parallelism (Bazel/Nx-style per-unit blocks) while keeping persistent
//! server/watch logs live, and caps each unit's buffered output so no single
//! unit buffers without limit. The cap is *per unit*: total channel memory
//! still scales with the number of units buffering concurrently, so a global
//! ceiling (if needed) is the APPLY exec layer's concern, not this channel's.
//!
//! Under [`PassingOutput::Hide`] a normal unit keeps only a bounded
//! [`TailBuffer`]: a passing unit prints nothing, and a failing unit prints its
//! last lines. With [`UnitLogs`] attached, every unit's full output is also
//! written to its own log file, whose path [`UnitOutputChannel::take_log`]
//! hands back for the unit's terminal event.

use std::collections::HashMap;
use std::path::PathBuf;

use rskit_errors::AppResult;
use toven_model::{OutputStream, UnitOutput};
use toven_ports::RawOutputSink;

use super::log::UnitLogs;
use super::tail::TailBuffer;

/// Default per-unit buffer cap before a normal unit's block is spilled early.
///
/// Generous enough that ordinary task output flushes as a single labeled block,
/// small enough that no single unit buffers without limit before spilling. This
/// caps *per-unit* buffering, not total channel memory — aggregate use still
/// scales with the number of units buffering concurrently. Past this cap a
/// unit's output is spilled as an extra block rather than buffered without
/// limit — see [`UnitOutputChannel::with_max_buffer_bytes`] for the trade-off.
const DEFAULT_MAX_BUFFER_BYTES: usize = 8 * 1024 * 1024;

/// How a unit's raw output is surfaced.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum OutputMode {
    /// Normal unit: buffer chunks, flush a labeled block on finish (plus an
    /// extra block early if the buffer spills past `max_buffer_bytes`).
    Buffered,
    /// Persistent unit: live-tail every chunk as it arrives.
    Live,
}

/// Whether a passing normal unit's captured output reaches the sink.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
pub enum PassingOutput {
    /// Flush every buffered unit's block, pass or fail.
    #[default]
    Show,
    /// Print nothing for a passing unit; print a failing unit's last lines.
    Hide,
}

/// How a unit ended, as far as its output is concerned.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum UnitOutcome {
    /// The unit passed (or was a cache hit); quiet output drops it.
    Passed,
    /// The unit failed, timed out or was cancelled; its output is shown.
    Failed,
}

/// Default number of trailing lines shown for a failing quiet unit.
pub const DEFAULT_FAILURE_TAIL_LINES: usize = 200;

/// Per-unit buffered state for a [`OutputMode::Buffered`] unit.
#[derive(Default)]
struct Buffer {
    chunks: Vec<UnitOutput>,
    bytes: usize,
}

/// Routes per-unit [`UnitOutput`] chunks to a [`RawOutputSink`] under the
/// buffer-normal / live-persistent policy.
///
/// Lifecycle: [`register`](Self::register) a unit with its mode,
/// [`push`](Self::push) chunks as they arrive (any interleaving across units is
/// fine), then [`finish`](Self::finish) to flush a buffered unit's block. Live
/// units stream on each `push` and need no flush. An unregistered unit defaults
/// to [`OutputMode::Buffered`] so output is never silently dropped.
pub struct UnitOutputChannel<S: RawOutputSink> {
    sink: S,
    modes: HashMap<String, OutputMode>,
    buffers: HashMap<String, Buffer>,
    tails: HashMap<String, TailBuffer>,
    max_buffer_bytes: usize,
    passing: PassingOutput,
    failure_tail_lines: usize,
    logs: Option<UnitLogs>,
    log_paths: HashMap<String, PathBuf>,
}

impl<S: RawOutputSink> UnitOutputChannel<S> {
    /// Create a channel writing through `sink` with the default buffer cap.
    pub fn new(sink: S) -> Self {
        Self::with_max_buffer_bytes(sink, DEFAULT_MAX_BUFFER_BYTES)
    }

    /// Create a channel with an explicit per-unit buffer cap (in bytes).
    ///
    /// When a buffered unit accumulates more than `max_buffer_bytes` it spills
    /// the accumulated chunks as a block immediately, bounding any single
    /// unit's buffer at the cost of splitting that unit's output across more
    /// than one block. The cap is per unit; total channel memory still scales
    /// with the number of units buffering concurrently.
    #[must_use]
    pub fn with_max_buffer_bytes(sink: S, max_buffer_bytes: usize) -> Self {
        Self {
            sink,
            modes: HashMap::new(),
            buffers: HashMap::new(),
            tails: HashMap::new(),
            max_buffer_bytes: max_buffer_bytes.max(1),
            passing: PassingOutput::Show,
            failure_tail_lines: DEFAULT_FAILURE_TAIL_LINES,
            logs: None,
            log_paths: HashMap::new(),
        }
    }

    /// Choose whether passing normal units print, and how many trailing lines
    /// a failing unit shows when they do not.
    #[must_use]
    pub fn with_passing_output(
        mut self,
        passing: PassingOutput,
        failure_tail_lines: usize,
    ) -> Self {
        self.passing = passing;
        self.failure_tail_lines = failure_tail_lines.max(1);
        self
    }

    /// Write every unit's full output to its own file in `logs`.
    #[must_use]
    pub fn with_logs(mut self, logs: UnitLogs) -> Self {
        self.logs = Some(logs);
        self
    }

    /// Whether passing normal units print nothing.
    #[must_use]
    pub fn hides_passing(&self) -> bool {
        self.passing == PassingOutput::Hide
    }

    /// The log file `unit_id` wrote. A finished unit hands back its closed
    /// log; a live unit that is still open (a persistent unit, which never
    /// [`finish`](Self::finish)es) flushes it and keeps appending late output.
    ///
    /// # Errors
    /// Returns an error if flushing an open log fails.
    pub fn take_log(&mut self, unit_id: &str) -> AppResult<Option<PathBuf>> {
        if let Some(path) = self.log_paths.remove(unit_id) {
            return Ok(Some(path));
        }
        self.logs
            .as_mut()
            .map_or_else(|| Ok(None), |logs| logs.flush(unit_id))
    }

    /// Whether the underlying sink de-interleaves concurrent live output by
    /// `unit_id`. When `true`, the APPLY spine may stream normal units live
    /// under parallelism, wrapping each in [`begin_unit`](Self::begin_unit) /
    /// [`end_unit`](Self::end_unit).
    #[must_use]
    pub fn supports_concurrent_live(&self) -> bool {
        self.sink.supports_concurrent_live()
    }

    /// Announce a live unit's start to the sink so a concurrent-live renderer
    /// can allocate its region.
    ///
    /// # Errors
    /// Propagates any [`RawOutputSink`] write failure.
    pub fn begin_unit(&mut self, unit_id: &str, label: &str) -> AppResult<()> {
        self.sink.begin_unit(unit_id, label)
    }

    /// Announce a live unit's terminal status to the sink so a concurrent-live
    /// renderer can collapse its region.
    ///
    /// # Errors
    /// Propagates any [`RawOutputSink`] write failure.
    pub fn end_unit(&mut self, unit_id: &str, status: toven_model::UnitStatus) -> AppResult<()> {
        self.sink.end_unit(unit_id, status)
    }

    /// Announce that the run has finished so the sink can emit an end-of-run
    /// epilogue (e.g. a consolidated failure section).
    ///
    /// # Errors
    /// Propagates any [`RawOutputSink`] write failure.
    pub fn finish_run(&mut self) -> AppResult<()> {
        self.sink.finish_run()
    }

    /// Declare how `unit_id`'s output should be surfaced.
    pub fn register(&mut self, unit_id: impl Into<String>, mode: OutputMode) {
        self.modes.insert(unit_id.into(), mode);
    }

    /// Route one raw output chunk.
    ///
    /// Live units stream immediately; buffered units accumulate (spilling a
    /// block if they exceed the cap). Unregistered units are treated as
    /// [`OutputMode::Buffered`].
    ///
    /// # Errors
    /// Propagates any [`RawOutputSink`] write failure.
    pub fn push(&mut self, output: UnitOutput) -> AppResult<()> {
        if let Some(logs) = self.logs.as_mut() {
            logs.append(&output.unit_id, &output.bytes)?;
        }
        match (self.mode_of(&output.unit_id), self.passing) {
            (OutputMode::Live, _) => self.sink.live(&output),
            (OutputMode::Buffered, PassingOutput::Show) => self.buffer(output),
            (OutputMode::Buffered, PassingOutput::Hide) => {
                let lines = self.failure_tail_lines;
                self.tails
                    .entry(output.unit_id.clone())
                    .or_insert_with(|| TailBuffer::new(output.unit_id.clone(), lines))
                    .push(&output);
                Ok(())
            }
        }
    }

    /// Flush `unit_id`'s buffered block (no-op for live or output-free units).
    ///
    /// Under [`PassingOutput::Hide`] a [`UnitOutcome::Passed`] unit prints
    /// nothing and a [`UnitOutcome::Failed`] unit prints its last lines. Either
    /// way the unit's log file (if any) is closed and its path kept for
    /// [`take_log`](Self::take_log).
    ///
    /// Contract: the APPLY exec layer must drain a unit's output before calling
    /// `finish` for it. `finish` clears the unit's registered mode, so a chunk
    /// that arrives *after* finish is treated as a fresh unregistered
    /// (buffered) unit and will only be flushed by a later `finish` — callers
    /// own the ordering, the channel does not buffer unboundedly to compensate.
    ///
    /// # Errors
    /// Propagates any [`RawOutputSink`] write failure.
    pub fn finish(&mut self, unit_id: &str, outcome: UnitOutcome) -> AppResult<()> {
        // Write before clearing state: if the sink write fails, the buffered chunks,
        // tail and mode stay intact so the caller can retry without losing output
        // (no success-shaped data loss).
        if outcome == UnitOutcome::Failed
            && let Some(tail) = self.tails.get(unit_id)
        {
            let mut chunks = Vec::new();
            let dropped = tail.dropped_lines();
            if dropped > 0 {
                chunks.push(UnitOutput {
                    unit_id: unit_id.to_string(),
                    stream: OutputStream::Stderr,
                    bytes: format!("… {dropped} earlier lines are in the unit log\n").into_bytes(),
                });
            }
            chunks.extend(tail.chunks());
            self.sink.block(unit_id, &chunks)?;
        }
        if let Some(buffer) = self.buffers.get(unit_id)
            && !buffer.chunks.is_empty()
        {
            self.sink.block(unit_id, &buffer.chunks)?;
        }
        if let Some(logs) = self.logs.as_mut()
            && let Some(path) = logs.close(unit_id)?
        {
            self.log_paths.insert(unit_id.to_string(), path);
        }
        self.tails.remove(unit_id);
        self.buffers.remove(unit_id);
        self.modes.remove(unit_id);
        Ok(())
    }

    fn mode_of(&self, unit_id: &str) -> OutputMode {
        self.modes
            .get(unit_id)
            .copied()
            .unwrap_or(OutputMode::Buffered)
    }

    fn buffer(&mut self, output: UnitOutput) -> AppResult<()> {
        let unit_id = output.unit_id.clone();
        let buffer = self.buffers.entry(unit_id.clone()).or_default();
        buffer.bytes = buffer.bytes.saturating_add(output.bytes.len());
        buffer.chunks.push(output);
        if buffer.bytes <= self.max_buffer_bytes {
            return Ok(());
        }
        // Over cap: spill as a block. Write before clearing so a sink failure keeps the
        // accumulated chunks buffered for retry rather than dropping them (no
        // success-shaped data loss).
        self.sink.block(&unit_id, &self.buffers[&unit_id].chunks)?;
        self.buffers.remove(&unit_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use toven_model::{OutputStream, UnitOutput};
    use toven_testkit::RecordingRawOutputSink;

    use super::{OutputMode, PassingOutput, UnitOutcome, UnitOutputChannel};
    use crate::output::UnitLogs;

    fn chunk(unit: &str, bytes: &[u8]) -> UnitOutput {
        UnitOutput {
            unit_id: unit.into(),
            stream: OutputStream::Stdout,
            bytes: bytes.to_vec(),
        }
    }

    #[test]
    fn buffered_unit_flushes_one_block_on_finish() {
        let sink = RecordingRawOutputSink::new();
        let mut channel = UnitOutputChannel::new(sink.clone());
        channel.register("u1", OutputMode::Buffered);
        channel.push(chunk("u1", b"a")).unwrap();
        channel.push(chunk("u1", b"b")).unwrap();
        channel.finish("u1", UnitOutcome::Passed).unwrap();

        assert!(sink.live_chunks().is_empty());
        assert_eq!(
            sink.blocks(),
            vec![("u1".to_string(), vec![chunk("u1", b"a"), chunk("u1", b"b")])]
        );
    }

    #[test]
    fn live_unit_streams_each_chunk_immediately() {
        let sink = RecordingRawOutputSink::new();
        let mut channel = UnitOutputChannel::new(sink.clone());
        channel.register("srv", OutputMode::Live);
        channel.push(chunk("srv", b"x")).unwrap();
        channel.push(chunk("srv", b"y")).unwrap();
        channel.finish("srv", UnitOutcome::Passed).unwrap();

        assert_eq!(
            sink.live_chunks(),
            vec![chunk("srv", b"x"), chunk("srv", b"y")]
        );
        assert!(sink.blocks().is_empty());
    }

    #[test]
    fn unregistered_unit_defaults_to_buffered() {
        let sink = RecordingRawOutputSink::new();
        let mut channel = UnitOutputChannel::new(sink.clone());
        channel.push(chunk("ghost", b"o")).unwrap();
        channel.finish("ghost", UnitOutcome::Passed).unwrap();

        assert!(sink.live_chunks().is_empty());
        assert_eq!(
            sink.blocks(),
            vec![("ghost".to_string(), vec![chunk("ghost", b"o")])]
        );
    }

    #[test]
    fn buffer_spills_past_cap_to_stay_bounded() {
        let sink = RecordingRawOutputSink::new();
        let mut channel = UnitOutputChannel::with_max_buffer_bytes(sink.clone(), 2);
        channel.register("u1", OutputMode::Buffered);
        channel.push(chunk("u1", b"abc")).unwrap(); // 3 bytes > cap → spill
        channel.push(chunk("u1", b"d")).unwrap();
        channel.finish("u1", UnitOutcome::Passed).unwrap();

        let blocks = sink.blocks();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].1, vec![chunk("u1", b"abc")]);
        assert_eq!(blocks[1].1, vec![chunk("u1", b"d")]);
    }

    #[test]
    fn finish_without_output_emits_no_block() {
        let sink = RecordingRawOutputSink::new();
        let mut channel = UnitOutputChannel::new(sink.clone());
        channel.register("u1", OutputMode::Buffered);
        channel.finish("u1", UnitOutcome::Passed).unwrap();
        assert!(sink.blocks().is_empty());
    }

    #[test]
    fn finish_failure_preserves_buffer_for_retry() {
        let sink = RecordingRawOutputSink::new();
        let mut channel = UnitOutputChannel::new(sink.clone());
        channel.register("u1", OutputMode::Buffered);
        channel.push(chunk("u1", b"a")).unwrap();
        channel.push(chunk("u1", b"b")).unwrap();

        sink.fail_blocks(true);
        assert!(channel.finish("u1", UnitOutcome::Passed).is_err());
        assert!(sink.blocks().is_empty(), "failed write must record nothing");

        // The buffered chunks survived the failed flush; a retry lands them.
        sink.fail_blocks(false);
        channel.finish("u1", UnitOutcome::Passed).unwrap();
        assert_eq!(
            sink.blocks(),
            vec![("u1".to_string(), vec![chunk("u1", b"a"), chunk("u1", b"b")])]
        );
    }

    #[test]
    fn spill_failure_preserves_buffer_for_retry() {
        let sink = RecordingRawOutputSink::new();
        let mut channel = UnitOutputChannel::with_max_buffer_bytes(sink.clone(), 2);
        channel.register("u1", OutputMode::Buffered);

        sink.fail_blocks(true);
        // 3 bytes > cap → spill attempt fails; chunk must stay buffered.
        assert!(channel.push(chunk("u1", b"abc")).is_err());
        assert!(sink.blocks().is_empty(), "failed spill must record nothing");

        // A later finish (after recovery) flushes the preserved chunk.
        sink.fail_blocks(false);
        channel.finish("u1", UnitOutcome::Passed).unwrap();
        assert_eq!(
            sink.blocks(),
            vec![("u1".to_string(), vec![chunk("u1", b"abc")])]
        );
    }

    fn quiet(sink: RecordingRawOutputSink) -> UnitOutputChannel<RecordingRawOutputSink> {
        UnitOutputChannel::new(sink).with_passing_output(PassingOutput::Hide, 2)
    }

    #[test]
    fn quiet_passing_unit_prints_nothing() {
        let sink = RecordingRawOutputSink::new();
        let mut channel = quiet(sink.clone());
        channel.register("u1", OutputMode::Buffered);
        channel.push(chunk("u1", b"noise\n")).unwrap();
        channel.finish("u1", UnitOutcome::Passed).unwrap();
        assert!(sink.blocks().is_empty());
        assert!(sink.live_chunks().is_empty());
    }

    #[test]
    fn quiet_failing_unit_prints_its_tail_and_notes_dropped_lines() {
        let sink = RecordingRawOutputSink::new();
        let mut channel = quiet(sink.clone());
        channel.register("u1", OutputMode::Buffered);
        channel.push(chunk("u1", b"a\nb\nc\n")).unwrap();
        channel.finish("u1", UnitOutcome::Failed).unwrap();
        let blocks = sink.blocks();
        assert_eq!(blocks.len(), 1);
        let text: String = blocks[0]
            .1
            .iter()
            .map(|c| String::from_utf8_lossy(&c.bytes).into_owned())
            .collect();
        assert_eq!(text, "… 1 earlier lines are in the unit log\nb\nc\n");
    }

    #[test]
    fn quiet_failure_write_error_keeps_the_tail_for_retry() {
        let sink = RecordingRawOutputSink::new();
        let mut channel = quiet(sink.clone());
        channel.register("u1", OutputMode::Buffered);
        channel.push(chunk("u1", b"boom\n")).unwrap();
        sink.fail_blocks(true);
        assert!(channel.finish("u1", UnitOutcome::Failed).is_err());
        sink.fail_blocks(false);
        channel.finish("u1", UnitOutcome::Failed).unwrap();
        assert_eq!(sink.blocks().len(), 1);
    }

    #[test]
    fn quiet_live_unit_still_streams() {
        let sink = RecordingRawOutputSink::new();
        let mut channel = quiet(sink.clone());
        channel.register("srv", OutputMode::Live);
        channel.push(chunk("srv", b"listening\n")).unwrap();
        assert_eq!(sink.live_chunks().len(), 1);
    }

    #[test]
    fn logs_capture_full_output_and_hand_back_the_path() {
        let root = rskit_fs::TempDir::new().unwrap();
        let logs = UnitLogs::create(root.path(), "run-1", 3).unwrap();
        let sink = RecordingRawOutputSink::new();
        let mut channel = quiet(sink.clone()).with_logs(logs);
        channel.register("u1", OutputMode::Buffered);
        channel.push(chunk("u1", b"a\nb\nc\n")).unwrap();
        channel.finish("u1", UnitOutcome::Passed).unwrap();
        let path = channel.take_log("u1").unwrap().expect("log path");
        assert_eq!(std::fs::read_to_string(path).unwrap(), "a\nb\nc\n");
        assert!(channel.take_log("u1").unwrap().is_none());
        assert!(sink.blocks().is_empty());
    }

    #[test]
    fn a_live_unit_hands_back_its_log_and_keeps_appending() {
        let root = rskit_fs::TempDir::new().unwrap();
        let logs = UnitLogs::create(root.path(), "run-1", 3).unwrap();
        let mut channel = quiet(RecordingRawOutputSink::new()).with_logs(logs);
        channel.register("srv", OutputMode::Live);
        channel.push(chunk("srv", b"ready\n")).unwrap();
        let path = channel.take_log("srv").unwrap().expect("log path");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "ready\n");
        channel.push(chunk("srv", b"bye\n")).unwrap();
        channel.finish("srv", UnitOutcome::Passed).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "ready\nbye\n");
    }
}
