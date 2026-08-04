//! [`Packager`] — the artifact-build phase contract (`package`).

use rskit_errors::AppResult;
use toven_model::Module;

use super::Artifact;

/// Build and verify a module's publishable artifact.
///
/// The `package` phase's ecosystem sliver. A native implementation invokes the
/// ecosystem's packaging tool argv-first; a delegated backing hands the phase to
/// an external tool while the engine keeps ownership of ordering, preview, and
/// reporting. Object-safe so the engine can hold it behind
/// [`ReleaseAdapter`](super::ReleaseAdapter).
pub trait Packager {
    /// Build and verify the publishable artifact.
    fn package(&self, module: &Module) -> AppResult<Artifact>;
}
