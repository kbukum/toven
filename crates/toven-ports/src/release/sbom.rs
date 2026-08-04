//! [`SbomProducer`] — the SBOM phase contract (`provenance`).

use std::path::Path;

use rskit_errors::AppResult;
use toven_model::Module;

use super::Artifact;

/// Generate a module's `CycloneDX` SBOM.
///
/// The SBOM sliver of the `provenance` phase. Orchestrated argv-first: the
/// engine owns scope, ordering, and reporting; this port only builds and runs
/// the tool invocation, bounded to `out_dir`. An ecosystem with no SBOM tooling
/// returns the typed "not applicable" (`Ok(None)`) via the default method, which
/// the engine records as a skipped module rather than a success-shaped empty
/// artifact. Object-safe so the engine can hold it behind
/// [`ReleaseAdapter`](super::ReleaseAdapter).
pub trait SbomProducer {
    /// Generate a `CycloneDX` SBOM for the module, writing it under `out_dir`.
    ///
    /// Orchestrated argv-first: the ecosystem's SBOM tool is invoked as an
    /// argument vector against the module's manifest, its output bounded to
    /// `out_dir`. Returns `Ok(None)` when the ecosystem has no SBOM tooling — a
    /// typed "not applicable" the engine records as a skipped module rather than
    /// a success-shaped empty artifact; the default is exactly that, so an
    /// adapter opts in by overriding.
    ///
    /// # Errors
    /// Propagates a tool spawn/IO failure or a non-zero tool exit.
    fn sbom(&self, module: &Module, out_dir: &Path) -> AppResult<Option<Artifact>> {
        let _ = (module, out_dir);
        Ok(None)
    }
}
