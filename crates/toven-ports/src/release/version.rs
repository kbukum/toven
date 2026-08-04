//! [`VersionSource`] — the version-read phase contract (`select`/`bump` seed).

use rskit_errors::AppResult;
use rskit_version::semver::Version;
use toven_model::Module;

/// Read a module's declared and published versions.
///
/// The version I/O sliver of the release seam: the engine owns the bump plan,
/// idempotency, and ordering; this port only reads the manifest version and
/// queries the registry for what it already reports as published. Object-safe so
/// the engine can hold it as a trait object behind
/// [`ReleaseAdapter`](super::ReleaseAdapter).
pub trait VersionSource {
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
}
