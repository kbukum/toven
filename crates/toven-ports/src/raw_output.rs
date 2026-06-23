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
use toven_model::UnitOutput;

/// Where a unit's raw output bytes land.
///
/// Implemented by the CLI's terminal-bound adapter (and by recording doubles in
/// tests). Call ordering is fully determined by the engine channel's policy:
/// `live` is called in arrival order for persistent units; `block` is called
/// once per flush for a normal unit, in finish order.
pub trait RawOutputSink {
    /// Render one live chunk from a persistent (live-tailed) unit, as it arrives.
    ///
    /// # Errors
    /// Propagates any sink write failure.
    fn live(&mut self, chunk: &UnitOutput) -> AppResult<()>;

    /// Render a complete, labeled block of buffered chunks for one normal unit.
    ///
    /// Called when the unit finishes (or, for bounded memory, when its buffer
    /// spills past the channel cap). `chunks` are in arrival order and all carry
    /// the same `unit_id`.
    ///
    /// # Errors
    /// Propagates any sink write failure.
    fn block(&mut self, unit_id: &str, chunks: &[UnitOutput]) -> AppResult<()>;
}
