//! The thin ecosystem release sliver — version I/O, packaging, and one publish
//! attempt; the generic ~90% (bump plan, ordering, retry) lives in the engine.

use std::path::Path;

use rskit_errors::AppResult;
use rskit_version::semver::Version;
use toven_model::Module;

use super::{Artifact, PublishOutcome, ReleaseCredentials, ReleaseMutation, TagScheme};

/// The ~10% ecosystem-specific release surface.
///
/// Object-safe so
/// [`ConfiguredAdapter::release_target`](crate::provider::ConfiguredAdapter::release_target)
/// can hand back a `Box<dyn ReleaseTarget>`. The engine owns change-detection,
/// the bump plan, topo order, changelog, tagging, idempotency, and the retry
/// loop; the port owns reading/writing the version, querying the registry,
/// packaging, release-tag grammar, and one classified publish attempt.
pub trait ReleaseTarget {
    /// Read the module's declared version from its manifest.
    fn declared_version(&self, module: &Module) -> AppResult<Version>;

    /// Query the registry for the versions it already reports as published —
    /// the publish loop's idempotency pre-skip and the tag seed.
    ///
    /// Best-effort by contract: an ecosystem whose registry CLI exposes only
    /// the latest published version may return just that one rather than the
    /// full set. The publish loop therefore treats a non-membership as "attempt
    /// and let the registry decide", with
    /// [`PublishOutcome::AlreadyPublished`](super::PublishOutcome::AlreadyPublished)
    /// as the authoritative idempotency backstop.
    fn published_versions(&self, module: &Module) -> AppResult<Vec<Version>>;

    /// Build this module's release-tag scheme, honoring a configured
    /// `tag_format` override (`None` = the target's ecosystem-default shape).
    fn tag_scheme(&self, module: &Module, tag_format: Option<&str>) -> AppResult<TagScheme>;

    /// Build and verify the publishable artifact.
    fn package(&self, module: &Module) -> AppResult<Artifact>;

    /// Apply one atomic version mutation to the module's manifest.
    fn apply_release(&self, module: &Module, mutation: &ReleaseMutation) -> AppResult<()>;

    /// Perform exactly one publish attempt and classify the registry's
    /// response.
    ///
    /// `credentials` carries the *name* of the registry-token environment
    /// variable (never the secret): a registry-publishing adapter reads that
    /// variable from its own environment at publish time and forwards the
    /// credential to its toolchain through the child process environment (never
    /// argv), while a tag-only target ignores it. A `None`
    /// [`registry_token_env`](ReleaseCredentials::registry_token_env) means
    /// "use the toolchain's ambient default credential".
    fn publish(
        &self,
        module: &Module,
        artifact: &Artifact,
        credentials: &ReleaseCredentials,
    ) -> AppResult<PublishOutcome>;

    /// Generate a `CycloneDX` SBOM for the module, writing it under `out_dir`.
    ///
    /// Orchestrated argv-first: the ecosystem's SBOM tool is invoked as an
    /// argument vector against the module's manifest, its output bounded to
    /// `out_dir`. The engine owns scope, ordering, and reporting; the target
    /// owns only building and running the tool invocation. Returns `Ok(None)`
    /// when the ecosystem has no SBOM tooling — a typed "not applicable" the
    /// engine records as a skipped module rather than a success-shaped empty
    /// artifact; the default is exactly that, so an adapter opts in by
    /// overriding.
    ///
    /// # Errors
    /// Propagates a tool spawn/IO failure or a non-zero tool exit.
    fn sbom(&self, module: &Module, out_dir: &Path) -> AppResult<Option<Artifact>> {
        let _ = (module, out_dir);
        Ok(None)
    }
}
