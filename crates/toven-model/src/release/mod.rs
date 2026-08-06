//! Release-flow vocabulary: the named [`ReleasePhase`] stages the engine drives
//! and the [`Entrypoint`] that says who cuts the release.
//!
//! Layer-0 vocabulary only — it *names* the release flow's stages and its
//! entrypoint so config, reporting, and the per-phase seam can refer to them,
//! and holds no behavior. The complementary `native | delegated` backing
//! concept (`PhaseBacking`) lives in `toven-ports`, the seam layer, since it
//! describes how a phase is satisfied rather than the flow's shape.

mod entrypoint;
mod phase;

pub use entrypoint::Entrypoint;
pub use phase::ReleasePhase;
