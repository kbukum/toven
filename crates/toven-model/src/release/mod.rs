//! Release-flow vocabulary: the named [`ReleasePhase`] stages the engine drives.
//!
//! Layer-0 vocabulary only — it *names* the release flow's stages so config,
//! reporting, and the per-phase seam can refer to them, and holds no behavior.
//! The complementary `native | delegated` backing concept (`PhaseBacking`)
//! lives in `toven-ports`, the seam layer, since it describes how a phase is
//! satisfied rather than the flow's shape.

mod phase;

pub use phase::ReleasePhase;
