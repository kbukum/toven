//! [`Publisher`] — the registry-publish phase contract (`publish`).

use rskit_errors::AppResult;
use toven_model::Module;

use super::{Artifact, PublishOutcome, ReleaseCredentials, Visibility};

/// Perform one classified registry-publish attempt.
///
/// The `publish` phase's ecosystem sliver: the engine owns the retry loop,
/// idempotency, and ordering; this port performs exactly one attempt and
/// classifies the registry's response. Object-safe so the engine can hold it
/// behind [`ReleaseAdapter`](super::ReleaseAdapter).
pub trait Publisher {
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
    ///
    /// `visibility` is the exposure the release is cut with. A registry that can
    /// only publish public versions (e.g. crates.io) **fails closed** with a
    /// typed error on any non-public visibility rather than silently publishing
    /// it publicly; a registry that supports the requested exposure creates the
    /// version accordingly.
    fn publish(
        &self,
        module: &Module,
        artifact: &Artifact,
        credentials: &ReleaseCredentials,
        visibility: Visibility,
    ) -> AppResult<PublishOutcome>;

    /// Check at plan time, before any mutation, that this target can publish
    /// `module` to `registry` with `visibility`.
    ///
    /// Called once per module whose resolved publication policy is a registry
    /// publish. A target rejects a registry value it cannot address or an
    /// exposure the registry cannot honor with a typed `release.registry` /
    /// `release.visibility` error, so the mismatch fails before a tag or
    /// commit. [`publish`](Self::publish) still enforces the same rules as the
    /// last line of defense. The default accepts every combination.
    ///
    /// # Errors
    /// A typed invalid-input error naming the rejected setting.
    fn validate_publication(
        &self,
        module: &Module,
        registry: &str,
        visibility: Visibility,
    ) -> AppResult<()> {
        let _ = (module, registry, visibility);
        Ok(())
    }
}
