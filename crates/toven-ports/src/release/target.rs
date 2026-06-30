//! The thin ecosystem release sliver — version I/O, packaging, and one publish
//! attempt; the generic ~90% (bump plan, ordering, retry) lives in the engine.

use rskit_errors::AppResult;
use rskit_version::semver::Version;
use toven_model::Module;

use super::{Artifact, PublishOutcome, ReleaseMutation};

/// The ~10% ecosystem-specific release surface.
///
/// Object-safe so [`ConfiguredAdapter::release_target`](crate::provider::ConfiguredAdapter::release_target)
/// can hand back a `Box<dyn ReleaseTarget>`. The engine owns change-detection,
/// the bump plan, topo order, changelog, tagging, idempotency, and the retry
/// loop; the port owns reading/writing the version, querying the registry,
/// packaging, and one classified publish attempt.
pub trait ReleaseTarget {
    /// Read the module's declared version from its manifest.
    fn declared_version(&self, module: &Module) -> AppResult<Version>;

    /// Query the registry for the versions it already reports as published —
    /// the publish loop's idempotency pre-skip and the tag seed.
    ///
    /// Best-effort by contract: an ecosystem whose registry CLI exposes only the
    /// latest published version may return just that one rather than the full
    /// set. The publish loop therefore treats a non-membership as "attempt and
    /// let the registry decide", with
    /// [`PublishOutcome::AlreadyPublished`](super::PublishOutcome::AlreadyPublished)
    /// as the authoritative idempotency backstop.
    fn published_versions(&self, module: &Module) -> AppResult<Vec<Version>>;

    /// Build and verify the publishable artifact.
    fn package(&self, module: &Module) -> AppResult<Artifact>;

    /// Apply one atomic version mutation to the module's manifest.
    fn apply_release(&self, module: &Module, mutation: &ReleaseMutation) -> AppResult<()>;

    /// Perform exactly one publish attempt and classify the registry's response.
    fn publish(&self, module: &Module, artifact: &Artifact) -> AppResult<PublishOutcome>;
}
