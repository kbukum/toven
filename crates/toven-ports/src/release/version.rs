//! [`VersionSource`] — the version-read phase contract (`select`/`bump` seed).

use rskit_errors::{AppError, AppResult};
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
    /// Read the module's declared version, or `None` when the module has no
    /// released version yet.
    ///
    /// A manifest ecosystem (Rust) always reports the version its manifest
    /// declares. A tag-only ecosystem (Go) has no manifest version: its
    /// declared version is the highest reachable release tag, and a module with
    /// **no** reachable release tag has never been released, so it reports
    /// `None` rather than failing closed. `None` is a first-class "never
    /// released" signal the bump decision resolves to an initial release (a
    /// brand-new module joining a lock-step set), driven by an explicit or
    /// lock-step target, and `release status` reports it as `unreleased` — it
    /// is not an error. Callers that genuinely need a concrete version (such
    /// as packaging, image rendering, and artifact verification of an
    /// already-released module) use
    /// [`declared_version_required`](Self::declared_version_required).
    fn declared_version(&self, module: &Module) -> AppResult<Option<Version>>;

    /// Read the module's declared version, failing closed when it has none.
    ///
    /// Downstream release execution paths that produce concrete artifacts (such
    /// as packaging, image rendering, and artifact verification) operate on an
    /// already-released module and require a concrete version; a `None` there is
    /// a genuine error with an actionable message telling the operator to
    /// supply a first release version. The bump planner and `release status`,
    /// by contrast, consult [`declared_version`](Self::declared_version)
    /// directly so a never-released module can be planned or reported as
    /// unreleased.
    ///
    /// # Errors
    /// Propagates a read failure, or fails closed when the module has no
    /// released version.
    fn declared_version_required(&self, module: &Module) -> AppResult<Version> {
        self.declared_version(module)?.ok_or_else(|| {
            AppError::invalid_input(
                "release.version",
                format!(
                    "module '{}' has no released version; supply an explicit first release \
                     version (for example `--set-version <version>`) before releasing it",
                    module.key()
                ),
            )
        })
    }

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

    /// Parse a module's declared version from raw manifest **contents**, or
    /// `None` when these contents alone do not determine one (no version field,
    /// or a workspace-inherited version whose workspace root is not in scope).
    ///
    /// Unlike [`declared_version`](Self::declared_version) — which reads the
    /// module's manifest from the working tree — this parses a historical
    /// manifest body the engine fetched at a tag's commit via
    /// [`VcsReader::file_at_ref`](crate::vcs::VcsReader::file_at_ref),
    /// so an umbrella-tag baseline can anchor each module on its **own** version
    /// at that commit rather than the umbrella tag's shared version. Contents
    /// that fail to parse as the ecosystem's manifest format are an error; a
    /// well-formed manifest with no resolvable version is `Ok(None)`, letting
    /// the caller fall back to the umbrella tag's own version.
    fn version_in_manifest(&self, manifest: &str) -> AppResult<Option<Version>>;
}
