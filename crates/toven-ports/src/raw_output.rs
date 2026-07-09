//! `RawOutputSink` — the raw child-output **output port**.
//!
//! A sibling of [`Reporter`](crate::Reporter): where `Reporter` renders the
//! typed [`Event`](toven_model::Event) stream, this port carries the coarse,
//! per-unit raw child output that deliberately never becomes per-line
//! vocabulary. The engine's `UnitOutputChannel` decides *when* and *how* output
//! is grouped (buffered block vs. live chunk); the sink decides *where* it lands
//! and how it is labeled. Keeping the sink a port preserves the "libraries do
//! not print" rule: the engine emits structured calls, the CLI adapter renders
//! them to a terminal stream.

use rskit_errors::AppResult;
use toven_model::{UnitOutput, UnitStatus};

/// Where a unit's raw output bytes land.
///
/// Implemented by the CLI's terminal-bound adapter (and by recording doubles in
/// tests). Call ordering is fully determined by the engine channel's policy:
/// `live` is called in arrival order for persistent units; `block` is called in
/// call order for a normal unit — once on finish, and additionally (before
/// finish) whenever the unit's buffer spills past the channel cap.
///
/// A sink that can de-interleave concurrent output spatially (one visual region
/// per unit) reports [`supports_concurrent_live`](Self::supports_concurrent_live)
/// and receives a [`begin_unit`](Self::begin_unit)/[`end_unit`](Self::end_unit)
/// lifecycle around each live unit; the engine then streams normal units through
/// `live` even under parallelism instead of buffering them into blocks. Sinks
/// that render to a single linear stream leave the default (`false`) and keep
/// the buffer-normal / live-persistent behavior.
pub trait RawOutputSink: Send {
    /// Render one live chunk from a persistent (live-tailed) unit, as it arrives.
    ///
    /// # Errors
    /// Propagates any sink write failure.
    fn live(&mut self, chunk: &UnitOutput) -> AppResult<()>;

    /// Render a complete, labeled block of buffered chunks for one normal unit.
    ///
    /// Called when the unit finishes (or, to bound that unit's buffer, when it
    /// spills past the per-unit channel cap). `chunks` are in arrival order and
    /// all carry the same `unit_id`.
    ///
    /// # Errors
    /// Propagates any sink write failure.
    fn block(&mut self, unit_id: &str, chunks: &[UnitOutput]) -> AppResult<()>;

    /// Whether this sink de-interleaves concurrent live output by `unit_id`.
    ///
    /// When `true`, the engine may stream normal units live (through `live`)
    /// while they run in parallel, wrapping each in
    /// [`begin_unit`](Self::begin_unit)/[`end_unit`](Self::end_unit). The default
    /// is `false`: the sink renders to a single linear stream and normal units
    /// are buffered into deterministic blocks.
    fn supports_concurrent_live(&self) -> bool {
        false
    }

    /// Announce that a live unit is starting, so a concurrent-live sink can
    /// allocate its region. `label` is a short human-facing identity for the
    /// region (typically derived from the unit id).
    ///
    /// The default is a no-op for single-stream sinks.
    ///
    /// # Errors
    /// Propagates any sink write failure.
    fn begin_unit(&mut self, unit_id: &str, label: &str) -> AppResult<()> {
        let _ = (unit_id, label);
        Ok(())
    }

    /// Announce that a live unit finished with `status`, so a concurrent-live
    /// sink can collapse its region to a verdict and flush the remainder to
    /// scrollback.
    ///
    /// The default is a no-op for single-stream sinks.
    ///
    /// # Errors
    /// Propagates any sink write failure.
    fn end_unit(&mut self, unit_id: &str, status: UnitStatus) -> AppResult<()> {
        let _ = (unit_id, status);
        Ok(())
    }
}

impl RawOutputSink for Box<dyn RawOutputSink> {
    fn live(&mut self, chunk: &UnitOutput) -> AppResult<()> {
        (**self).live(chunk)
    }

    fn block(&mut self, unit_id: &str, chunks: &[UnitOutput]) -> AppResult<()> {
        (**self).block(unit_id, chunks)
    }

    fn supports_concurrent_live(&self) -> bool {
        (**self).supports_concurrent_live()
    }

    fn begin_unit(&mut self, unit_id: &str, label: &str) -> AppResult<()> {
        (**self).begin_unit(unit_id, label)
    }

    fn end_unit(&mut self, unit_id: &str, status: UnitStatus) -> AppResult<()> {
        (**self).end_unit(unit_id, status)
    }
}
