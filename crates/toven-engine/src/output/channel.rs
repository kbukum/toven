//! [`UnitOutputChannel`] — buffers normal units, live-tails persistent ones.
//!
//! The channel is the engine-owned policy layer between the APPLY exec loop and
//! a [`RawOutputSink`]. It groups raw output deterministically under parallelism
//! (Bazel/Nx-style per-unit blocks) while keeping persistent server/watch logs
//! live, and caps each unit's buffered output so no single unit buffers without
//! limit. The cap is *per unit*: total channel memory still scales with the
//! number of units buffering concurrently, so a global ceiling (if needed) is
//! the APPLY exec layer's concern, not this channel's.

use std::collections::HashMap;

use rskit_errors::AppResult;
use toven_model::UnitOutput;
use toven_ports::RawOutputSink;

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

/// Per-unit buffered state for a [`OutputMode::Buffered`] unit.
#[derive(Default)]
struct Buffer {
    chunks: Vec<UnitOutput>,
    bytes: usize,
}

/// Routes per-unit [`UnitOutput`] chunks to a [`RawOutputSink`] under the
/// buffer-normal / live-persistent policy (event-report Decision C).
///
/// Lifecycle: [`register`](Self::register) a unit with its mode, [`push`](Self::push)
/// chunks as they arrive (any interleaving across units is fine), then
/// [`finish`](Self::finish) to flush a buffered unit's block. Live units stream
/// on each `push` and need no flush. An unregistered unit defaults to
/// [`OutputMode::Buffered`] so output is never silently dropped.
pub struct UnitOutputChannel<S: RawOutputSink> {
    sink: S,
    modes: HashMap<String, OutputMode>,
    buffers: HashMap<String, Buffer>,
    max_buffer_bytes: usize,
}

impl<S: RawOutputSink> UnitOutputChannel<S> {
    /// Create a channel writing through `sink` with the default buffer cap.
    pub fn new(sink: S) -> Self {
        Self::with_max_buffer_bytes(sink, DEFAULT_MAX_BUFFER_BYTES)
    }

    /// Create a channel with an explicit per-unit buffer cap (in bytes).
    ///
    /// When a buffered unit accumulates more than `max_buffer_bytes` it spills
    /// the accumulated chunks as a block immediately, bounding any single unit's
    /// buffer at the cost of splitting that unit's output across more than one
    /// block. The cap is per unit; total channel memory still scales with the
    /// number of units buffering concurrently.
    #[must_use]
    pub fn with_max_buffer_bytes(sink: S, max_buffer_bytes: usize) -> Self {
        Self {
            sink,
            modes: HashMap::new(),
            buffers: HashMap::new(),
            max_buffer_bytes: max_buffer_bytes.max(1),
        }
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
        match self.mode_of(&output.unit_id) {
            OutputMode::Live => self.sink.live(&output),
            OutputMode::Buffered => self.buffer(output),
        }
    }

    /// Flush `unit_id`'s buffered block (no-op for live or output-free units).
    ///
    /// Contract: the APPLY exec layer (step 8) must drain a unit's output before
    /// calling `finish` for it. `finish` clears the unit's registered mode, so a
    /// chunk that arrives *after* finish is treated as a fresh unregistered
    /// (buffered) unit and will only be flushed by a later `finish` — callers
    /// own the ordering, the channel does not buffer unboundedly to compensate.
    ///
    /// # Errors
    /// Propagates any [`RawOutputSink`] write failure.
    pub fn finish(&mut self, unit_id: &str) -> AppResult<()> {
        // Flush before clearing state: if the sink write fails the buffered
        // chunks and mode stay intact so the caller can retry without losing
        // output (no success-shaped data loss).
        if let Some(buffer) = self.buffers.get(unit_id)
            && !buffer.chunks.is_empty()
        {
            self.sink.block(unit_id, &buffer.chunks)?;
        }
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
        // Over cap: spill as a block. Write before clearing so a sink failure
        // keeps the accumulated chunks buffered for retry rather than dropping
        // them (no success-shaped data loss).
        self.sink.block(&unit_id, &self.buffers[&unit_id].chunks)?;
        self.buffers.remove(&unit_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use toven_model::{OutputStream, UnitOutput};
    use toven_testkit::RecordingRawOutputSink;

    use super::{OutputMode, UnitOutputChannel};

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
        channel.finish("u1").unwrap();

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
        channel.finish("srv").unwrap();

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
        channel.finish("ghost").unwrap();

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
        channel.finish("u1").unwrap();

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
        channel.finish("u1").unwrap();
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
        assert!(channel.finish("u1").is_err());
        assert!(sink.blocks().is_empty(), "failed write must record nothing");

        // The buffered chunks survived the failed flush; a retry lands them.
        sink.fail_blocks(false);
        channel.finish("u1").unwrap();
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
        channel.finish("u1").unwrap();
        assert_eq!(
            sink.blocks(),
            vec![("u1".to_string(), vec![chunk("u1", b"abc")])]
        );
    }
}
