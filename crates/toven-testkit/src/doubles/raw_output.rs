//! [`RecordingRawOutputSink`] — a [`RawOutputSink`] that captures the channel's
//! `live`/`block` calls so tests can assert grouping and ordering.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_model::UnitOutput;
use toven_ports::RawOutputSink;

/// The recorded calls behind a [`RecordingRawOutputSink`] handle.
#[derive(Debug, Default)]
struct Recorded {
    live: Vec<UnitOutput>,
    blocks: Vec<(String, Vec<UnitOutput>)>,
}

/// A [`RawOutputSink`] that records every routed chunk in call order.
///
/// Live chunks land in [`live_chunks`](Self::live_chunks) (arrival order) and
/// flushed blocks in [`blocks`](Self::blocks) (finish order), so tests can
/// assert the buffer-normal / live-persistent policy without a terminal.
///
/// The recorder shares its state through an [`Arc`] so a test can keep a handle
/// to inspect after moving a [`clone`](Clone::clone) into the channel that owns
/// the sink — no recover-by-value escape hatch on the channel is required.
///
/// [`fail_blocks`](Self::fail_blocks) toggles a write failure on `block` so
/// tests can assert the channel preserves buffered data when a sink write fails.
#[derive(Debug, Clone, Default)]
pub struct RecordingRawOutputSink {
    inner: Arc<Mutex<Recorded>>,
    fail_block: Arc<AtomicBool>,
}

impl RecordingRawOutputSink {
    /// Construct an empty recorder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Make subsequent `block` writes fail (`true`) or succeed (`false`).
    ///
    /// A failing `block` records nothing, mirroring a real sink whose write
    /// did not land, so a test can assert the channel kept the data and that a
    /// later retry (after toggling back to `false`) flushes it.
    pub fn fail_blocks(&self, fail: bool) {
        self.fail_block.store(fail, Ordering::SeqCst);
    }

    /// The live-tailed chunks, in arrival order.
    #[must_use]
    pub fn live_chunks(&self) -> Vec<UnitOutput> {
        self.inner
            .lock()
            .expect("RecordingRawOutputSink mutex poisoned")
            .live
            .clone()
    }

    /// The flushed blocks as `(unit_id, chunks)`, in finish order.
    #[must_use]
    pub fn blocks(&self) -> Vec<(String, Vec<UnitOutput>)> {
        self.inner
            .lock()
            .expect("RecordingRawOutputSink mutex poisoned")
            .blocks
            .clone()
    }
}

impl RawOutputSink for RecordingRawOutputSink {
    fn live(&mut self, chunk: &UnitOutput) -> AppResult<()> {
        self.inner
            .lock()
            .expect("RecordingRawOutputSink mutex poisoned")
            .live
            .push(chunk.clone());
        Ok(())
    }

    fn block(&mut self, unit_id: &str, chunks: &[UnitOutput]) -> AppResult<()> {
        if self.fail_block.load(Ordering::SeqCst) {
            return Err(AppError::new(
                ErrorCode::ExternalService,
                "RecordingRawOutputSink: injected block failure",
            ));
        }
        self.inner
            .lock()
            .expect("RecordingRawOutputSink mutex poisoned")
            .blocks
            .push((unit_id.to_string(), chunks.to_vec()));
        Ok(())
    }
}
