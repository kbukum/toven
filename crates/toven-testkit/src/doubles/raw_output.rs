//! [`RecordingRawOutputSink`] — a [`RawOutputSink`] that captures the channel's
//! `live`/`block` calls so tests can assert grouping and ordering.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_model::{UnitOutput, UnitStatus};
use toven_ports::RawOutputSink;

/// The recorded calls behind a [`RecordingRawOutputSink`] handle.
#[derive(Debug, Default)]
struct Recorded {
    live: Vec<UnitOutput>,
    blocks: Vec<(String, Vec<UnitOutput>)>,
    begins: Vec<(String, String)>,
    ends: Vec<(String, UnitStatus)>,
}

/// A [`RawOutputSink`] that records every routed chunk in call order.
///
/// Live chunks land in [`live_chunks`](Self::live_chunks) (arrival order) and
/// flushed blocks in [`blocks`](Self::blocks) (`block` call order — one per
/// finish, plus any early spill blocks), so tests can assert the
/// buffer-normal / live-persistent policy without a terminal.
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
    concurrent_live: Arc<AtomicBool>,
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

    /// Advertise concurrent-live support (`true`) so the engine drives normal
    /// units through the `live` + `begin_unit`/`end_unit` lifecycle instead of
    /// buffering them into blocks. Defaults to `false`.
    pub fn set_concurrent_live(&self, supported: bool) {
        self.concurrent_live.store(supported, Ordering::SeqCst);
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

    /// The flushed blocks as `(unit_id, chunks)`, in `block` call order (one per
    /// finish, plus any early spill blocks).
    #[must_use]
    pub fn blocks(&self) -> Vec<(String, Vec<UnitOutput>)> {
        self.inner
            .lock()
            .expect("RecordingRawOutputSink mutex poisoned")
            .blocks
            .clone()
    }

    /// The `begin_unit` calls as `(unit_id, label)`, in call order.
    #[must_use]
    pub fn begins(&self) -> Vec<(String, String)> {
        self.inner
            .lock()
            .expect("RecordingRawOutputSink mutex poisoned")
            .begins
            .clone()
    }

    /// The `end_unit` calls as `(unit_id, status)`, in call order.
    #[must_use]
    pub fn ends(&self) -> Vec<(String, UnitStatus)> {
        self.inner
            .lock()
            .expect("RecordingRawOutputSink mutex poisoned")
            .ends
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

    fn supports_concurrent_live(&self) -> bool {
        self.concurrent_live.load(Ordering::SeqCst)
    }

    fn begin_unit(&mut self, unit_id: &str, label: &str) -> AppResult<()> {
        self.inner
            .lock()
            .expect("RecordingRawOutputSink mutex poisoned")
            .begins
            .push((unit_id.to_string(), label.to_string()));
        Ok(())
    }

    fn end_unit(&mut self, unit_id: &str, status: UnitStatus) -> AppResult<()> {
        self.inner
            .lock()
            .expect("RecordingRawOutputSink mutex poisoned")
            .ends
            .push((unit_id.to_string(), status));
        Ok(())
    }
}
