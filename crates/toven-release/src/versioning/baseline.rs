//! Release baseline resolution.
//!
//! Turns a [`BaselineSource`] policy choice into a concrete
//! [`ReleaseBaseline`] — the version an idempotency check anchors on plus, when
//! a commit is available, the diff ref change detection compares files against.
//! The resolver is the single home of the three-anchor policy (own tag,
//! umbrella tag, registry) so change detection (`change.rs`) selects a source
//! and delegates the mechanics here.
//!
//! Manifest parsing stays in the ecosystem adapters: an umbrella tag anchors
//! each module on its **own** declared version at that tag's commit — read
//! through [`VcsReader::file_at_ref`] and parsed by
//! [`VersionSource::version_in_manifest`], never a manifest re-parse in the
//! release engine — so a workspace that versions its modules independently
//! under one shared tag anchors each module correctly. When a module has no
//! manifest at that commit (or declares a version the contents can't resolve),
//! the baseline falls back to the umbrella tag's own version.

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_version::semver::Version;
use toven_model::Module;
use toven_ports::{Oid, TagRef, TagScheme, VcsReader, VersionSource};

use toven_semver::latest_matching;

use crate::ReleaseBaseline;
use crate::model::BaselineSource;

/// Byte budget for reading a module manifest at a historical commit.
///
/// Bounds the repository-controlled blob read behind
/// [`VcsReader::file_at_ref`] so an oversized historical manifest is rejected
/// during planning rather than materialized into memory. Matches the 4 MiB cap
/// the ecosystem adapters apply to working-tree manifest reads.
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;

/// Resolve a module's release baseline from the selected [`BaselineSource`].
///
/// - [`OwnTag`](BaselineSource::OwnTag) selects the latest tag matching the
///   module's own scheme; the baseline carries that tag's version and commit.
/// - [`UmbrellaTag`](BaselineSource::UmbrellaTag) selects the latest tag
///   matching the shared umbrella scheme, but anchors the version on the
///   module's **own** declared version at that tag's commit (falling back to the
///   umbrella tag's version when the module has no resolvable version there).
/// - [`Registry`](BaselineSource::Registry) anchors the version on the
///   registry's max published version, takes the diff ref from an inner tag
///   anchor, and uses the **max** of the two versions — the
///   `max(registry, version-at-tag)` composition. A registry lookup failure
///   downgrades to the inner tag anchor and never aborts.
///
/// Either tag path is an initial release when no tag matches.
///
/// # Errors
/// Propagates a typed VCS failure from reading a module's manifest at the
/// umbrella tag commit, or a manifest-parse failure from the adapter. Registry
/// failures downgrade rather than propagate.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn resolve_baseline(
    module: &Module,
    source: &BaselineSource,
    reader: &dyn VcsReader,
    version_source: &dyn VersionSource,
    tags: &[TagRef],
) -> AppResult<ReleaseBaseline> {
    match source {
        BaselineSource::OwnTag { scheme } => Ok(resolve_tag_anchor(module, scheme, tags)),
        BaselineSource::UmbrellaTag {
            umbrella_scheme: scheme,
        } => resolve_umbrella_anchor(module, scheme, reader, version_source, tags),
        BaselineSource::Registry { diff } => {
            resolve_registry(module, diff, reader, version_source, tags)
        }
    }
}

/// Resolve a tag-anchored baseline: the latest tag matching `scheme` supplies
/// the version (idempotency anchor) and its commit (diff ref); no matching tag
/// is an initial release.
fn resolve_tag_anchor(module: &Module, scheme: &TagScheme, tags: &[TagRef]) -> ReleaseBaseline {
    latest_matching(scheme, tags).map_or_else(
        || ReleaseBaseline::initial(module.key()),
        |(version, tag)| ReleaseBaseline::tag(module.key(), tag.name, version, tag.target),
    )
}

/// Resolve an umbrella-tag-anchored baseline: the latest tag matching the shared
/// umbrella `scheme` supplies the diff ref (its commit), but the version anchor
/// is the module's **own** declared version at that commit.
///
/// An umbrella workspace can version each module independently under one shared
/// tag, so the tag's own version is not the per-module release anchor. The
/// module's manifest at the tag commit is the authority; the umbrella tag's own
/// version is the fallback only when that manifest is absent (a module
/// introduced after the tag) or declares a version the contents cannot resolve
/// (e.g. workspace-inherited). No matching umbrella tag is an initial release.
fn resolve_umbrella_anchor(
    module: &Module,
    scheme: &TagScheme,
    reader: &dyn VcsReader,
    version_source: &dyn VersionSource,
    tags: &[TagRef],
) -> AppResult<ReleaseBaseline> {
    let Some((tag_version, tag)) = latest_matching(scheme, tags) else {
        return Ok(ReleaseBaseline::initial(module.key()));
    };
    let version =
        module_version_at(module, &tag.target, reader, version_source)?.unwrap_or(tag_version);
    Ok(ReleaseBaseline::tag(
        module.key(),
        tag.name,
        version,
        tag.target,
    ))
}

/// Read a module's declared version from its manifest **at a commit**, or `None`
/// when the module has no configured manifest, no manifest at that commit, or a
/// version the adapter cannot resolve from the manifest body alone.
fn module_version_at(
    module: &Module,
    commit: &Oid,
    reader: &dyn VcsReader,
    version_source: &dyn VersionSource,
) -> AppResult<Option<Version>> {
    let Some(manifest) = module.manifest.as_ref() else {
        return Ok(None);
    };
    let Some(bytes) =
        reader.file_at_ref(commit.as_str(), manifest.as_path(), MAX_MANIFEST_BYTES)?
    else {
        return Ok(None);
    };
    let text = String::from_utf8(bytes).map_err(|error| {
        AppError::new(
            ErrorCode::InvalidFormat,
            format!(
                "manifest '{}' at '{}' is not valid UTF-8",
                manifest.as_path().display(),
                commit.as_str()
            ),
        )
        .with_cause(error)
    })?;
    version_source.version_in_manifest(&text)
}

/// Resolve a registry-anchored baseline: the registry's max published version
/// anchors idempotency, the diff ref comes from the inner tag anchor, and the
/// effective version is the max of the two. A registry lookup failure downgrades
/// to the inner tag anchor rather than aborting — the publish loop's
/// `AlreadyPublished` classification remains the authoritative backstop, so a
/// transient registry outage must not fail change detection.
fn resolve_registry(
    module: &Module,
    diff: &BaselineSource,
    reader: &dyn VcsReader,
    version_source: &dyn VersionSource,
    tags: &[TagRef],
) -> AppResult<ReleaseBaseline> {
    let diff_baseline = resolve_baseline(module, diff, reader, version_source, tags)?;
    let registry_version = version_source
        .published_versions(module)
        .ok()
        .and_then(|versions| versions.into_iter().max());
    let version = max_version(registry_version, diff_baseline.version);
    Ok(ReleaseBaseline::anchored(
        module.key(),
        diff_baseline.tag,
        version,
        diff_baseline.target,
    ))
}

/// The higher of two optional versions, or whichever is present.
fn max_version(left: Option<Version>, right: Option<Version>) -> Option<Version> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (some, None) | (None, some) => some,
    }
}

#[cfg(test)]
mod tests {
    use rskit_version::semver::Version;
    use toven_model::{EcosystemId, Module, ModuleRef, RepoPath};
    use toven_ports::{Oid, TagRef, TagScheme};
    use toven_testkit::{FakeReleaseTarget, FakeVcsReader};

    use super::{max_version, resolve_baseline};
    use crate::model::BaselineSource;

    fn module(name: &str, root: &str) -> Module {
        Module::new(
            ModuleRef::new(EcosystemId::new("rust").expect("ecosystem"), name).expect("ref"),
            RepoPath::new(root).expect("path"),
        )
    }

    fn tag(name: &str, oid: &str) -> TagRef {
        TagRef::new(name, Oid::new(oid))
    }

    #[test]
    fn own_tag_selects_the_latest_matching_tag() {
        let module = module("core", "crates/core");
        let source = BaselineSource::own_tag(TagScheme::new("rust/core@", ""));
        let version_source = FakeReleaseTarget::new();
        let tags = vec![
            tag("rust/core@0.1.0", "a"),
            tag("rust/core@0.2.0", "b"),
            tag("rust/other@9.9.9", "c"),
        ];

        let reader = FakeVcsReader::new();
        let baseline =
            resolve_baseline(&module, &source, &reader, &version_source, &tags).expect("resolve");

        assert!(!baseline.is_initial());
        assert_eq!(baseline.version, Some(Version::new(0, 2, 0)));
        assert_eq!(baseline.tag.as_deref(), Some("rust/core@0.2.0"));
        assert_eq!(baseline.target.as_ref().map(Oid::as_str), Some("b"));
    }

    #[test]
    fn own_tag_without_a_matching_tag_is_an_initial_release() {
        let module = module("core", "crates/core");
        let source = BaselineSource::own_tag(TagScheme::new("rust/core@", ""));
        let version_source = FakeReleaseTarget::new();
        let tags = vec![tag("rust/other@1.0.0", "a")];

        let reader = FakeVcsReader::new();
        let baseline =
            resolve_baseline(&module, &source, &reader, &version_source, &tags).expect("resolve");

        assert!(baseline.is_initial());
        assert_eq!(baseline.version, None);
        assert_eq!(baseline.target, None);
    }

    #[test]
    fn umbrella_tag_falls_back_to_the_tag_version_without_a_module_manifest() {
        // The module's own scheme (rust/core@) never matches; the umbrella scheme
        // (v) does. With no module manifest to read at the tag commit, the
        // baseline falls back to the version the umbrella tag denotes.
        let module = module("core", "crates/core");
        let source = BaselineSource::umbrella_tag(TagScheme::new("v", ""));
        let version_source = FakeReleaseTarget::new();
        let tags = vec![
            tag("rust/core@0.1.0", "own"),
            tag("v1.3.0", "umbrella-old"),
            tag("v1.4.0", "umbrella"),
        ];

        let reader = FakeVcsReader::new();
        let baseline =
            resolve_baseline(&module, &source, &reader, &version_source, &tags).expect("resolve");

        assert!(!baseline.is_initial());
        assert_eq!(baseline.version, Some(Version::new(1, 4, 0)));
        assert_eq!(baseline.tag.as_deref(), Some("v1.4.0"));
        assert_eq!(baseline.target.as_ref().map(Oid::as_str), Some("umbrella"));
    }

    #[test]
    fn umbrella_tag_anchors_on_the_module_version_at_the_tag_commit() {
        // The key independent-versioning case: the umbrella tag denotes 1.4.0,
        // but the module declares its OWN version (0.2.0) in its manifest at that
        // commit. The baseline anchors on the module's own version, not the
        // shared tag version — the diff ref is still the umbrella tag commit.
        let mut module = module("core", "crates/core");
        module.manifest = Some(RepoPath::new("crates/core/Cargo.toml").expect("manifest path"));
        let source = BaselineSource::umbrella_tag(TagScheme::new("v", ""));
        let version_source = FakeReleaseTarget::new();
        let tags = vec![tag("v1.4.0", "umbrella")];
        let reader = FakeVcsReader::new().with_file_at_ref(
            "umbrella",
            "crates/core/Cargo.toml",
            "[package]\nname = \"core\"\nversion = \"0.2.0\"\n",
        );

        let baseline =
            resolve_baseline(&module, &source, &reader, &version_source, &tags).expect("resolve");

        assert!(!baseline.is_initial());
        assert_eq!(
            baseline.version,
            Some(Version::new(0, 2, 0)),
            "the anchor is the module's own version at the tag commit, not the umbrella version"
        );
        assert_eq!(baseline.tag.as_deref(), Some("v1.4.0"));
        assert_eq!(baseline.target.as_ref().map(Oid::as_str), Some("umbrella"));
    }

    #[test]
    fn registry_and_umbrella_take_the_higher_of_registry_and_module_version() {
        // Registry+umbrella for an independently-versioned crate: the registry
        // reports 0.2.0 published and the module declares 0.2.0 at the umbrella
        // commit, so the anchor is 0.2.0 — never the umbrella tag's own 1.4.0.
        let mut module = module("core", "crates/core");
        module.manifest = Some(RepoPath::new("crates/core/Cargo.toml").expect("manifest path"));
        let source =
            BaselineSource::registry(BaselineSource::umbrella_tag(TagScheme::new("v", "")));
        let version_source =
            FakeReleaseTarget::new().with_published_versions(vec![Version::new(0, 2, 0)]);
        let tags = vec![tag("v1.4.0", "umbrella")];
        let reader = FakeVcsReader::new().with_file_at_ref(
            "umbrella",
            "crates/core/Cargo.toml",
            "[package]\nname = \"core\"\nversion = \"0.2.0\"\n",
        );

        let baseline =
            resolve_baseline(&module, &source, &reader, &version_source, &tags).expect("resolve");

        assert_eq!(
            baseline.version,
            Some(Version::new(0, 2, 0)),
            "neither the registry nor the module version is the umbrella tag's 1.4.0"
        );
        assert_eq!(baseline.target.as_ref().map(Oid::as_str), Some("umbrella"));
    }

    #[test]
    fn registry_anchors_on_the_max_published_version() {
        // Registry is ahead of the diff tag: the published max anchors the
        // version, while the diff ref still comes from the umbrella tag commit.
        let module = module("core", "crates/core");
        let source =
            BaselineSource::registry(BaselineSource::umbrella_tag(TagScheme::new("v", "")));
        let version_source = FakeReleaseTarget::new()
            .with_published_versions(vec![Version::new(1, 0, 0), Version::new(1, 2, 0)]);
        let tags = vec![tag("v1.1.0", "umbrella")];

        let reader = FakeVcsReader::new();
        let baseline =
            resolve_baseline(&module, &source, &reader, &version_source, &tags).expect("resolve");

        assert_eq!(baseline.version, Some(Version::new(1, 2, 0)));
        assert_eq!(baseline.target.as_ref().map(Oid::as_str), Some("umbrella"));
    }

    #[test]
    fn registry_takes_the_higher_of_registry_and_tag_versions() {
        // The umbrella tag is ahead of the registry: the max composition keeps
        // the tag version so a crate released by tag but not yet indexed by the
        // registry is not treated as behind.
        let module = module("core", "crates/core");
        let source =
            BaselineSource::registry(BaselineSource::umbrella_tag(TagScheme::new("v", "")));
        let version_source =
            FakeReleaseTarget::new().with_published_versions(vec![Version::new(1, 0, 0)]);
        let tags = vec![tag("v1.3.0", "umbrella")];

        let reader = FakeVcsReader::new();
        let baseline =
            resolve_baseline(&module, &source, &reader, &version_source, &tags).expect("resolve");

        assert_eq!(baseline.version, Some(Version::new(1, 3, 0)));
        assert_eq!(baseline.target.as_ref().map(Oid::as_str), Some("umbrella"));
    }

    #[test]
    fn registry_lookup_failure_downgrades_to_the_tag_anchor() {
        // A registry outage must not abort: the baseline downgrades to the diff
        // tag anchor (version and commit) and still resolves.
        let module = module("core", "crates/core");
        let source =
            BaselineSource::registry(BaselineSource::umbrella_tag(TagScheme::new("v", "")));
        let version_source = FakeReleaseTarget::new().with_version_read_failure("registry offline");
        let tags = vec![tag("v1.1.0", "umbrella")];

        let reader = FakeVcsReader::new();
        let baseline =
            resolve_baseline(&module, &source, &reader, &version_source, &tags).expect("resolve");

        assert_eq!(baseline.version, Some(Version::new(1, 1, 0)));
        assert_eq!(baseline.target.as_ref().map(Oid::as_str), Some("umbrella"));
        assert!(!baseline.is_initial());
    }

    #[test]
    fn registry_without_a_diff_tag_still_anchors_on_the_registry_version() {
        // No tag has been cut yet, but the registry reports a published version:
        // the baseline anchors idempotency on it (not an initial release) even
        // though there is no commit to diff files against.
        let module = module("core", "crates/core");
        let source =
            BaselineSource::registry(BaselineSource::own_tag(TagScheme::new("rust/core@", "")));
        let version_source =
            FakeReleaseTarget::new().with_published_versions(vec![Version::new(2, 0, 0)]);
        let tags = vec![tag("rust/other@1.0.0", "a")];

        let reader = FakeVcsReader::new();
        let baseline =
            resolve_baseline(&module, &source, &reader, &version_source, &tags).expect("resolve");

        assert!(!baseline.is_initial());
        assert_eq!(baseline.version, Some(Version::new(2, 0, 0)));
        assert_eq!(baseline.target, None);
    }

    #[test]
    fn max_version_prefers_the_higher_and_the_present() {
        assert_eq!(
            max_version(Some(Version::new(1, 0, 0)), Some(Version::new(1, 2, 0))),
            Some(Version::new(1, 2, 0))
        );
        assert_eq!(
            max_version(Some(Version::new(1, 0, 0)), None),
            Some(Version::new(1, 0, 0))
        );
        assert_eq!(
            max_version(None, Some(Version::new(1, 0, 0))),
            Some(Version::new(1, 0, 0))
        );
        assert_eq!(max_version(None, None), None);
    }
}
