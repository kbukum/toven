//! Release baseline GATHER.
//!
//! Reads the impure facts a module's baseline needs — the module's **own**
//! declared version at the umbrella tag commit (via [`VcsReader::file_at_ref`] +
//! [`VersionSource::version_in_manifest`]) and, for a registry-anchored source,
//! the module's registry-published versions — and hands them to the pure
//! [`toven_version::resolve_baseline`] decision. All git/registry I/O happens
//! here, before the pure resolver, so the three-anchor policy (own tag, umbrella
//! tag, registry) stays a pure function of pre-gathered data.

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_version::semver::Version;
use toven_model::Module;
use toven_ports::{Oid, TagRef, TagScheme, VcsReader, VersionSource};

use toven_semver::latest_matching;
use toven_version::{BaselineSource, ReleaseBaseline, resolve_baseline as decide_baseline};

/// Byte budget for reading a module manifest at a historical commit.
///
/// Bounds the repository-controlled blob read behind
/// [`VcsReader::file_at_ref`] so an oversized historical manifest is rejected
/// during planning rather than materialized into memory. Matches the 4 MiB cap
/// the ecosystem adapters apply to working-tree manifest reads.
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;

/// Resolve a module's release baseline: gather the impure inputs the selected
/// [`BaselineSource`] needs, then decide the baseline purely.
///
/// The module's own version at the umbrella tag commit is read only when the
/// source anchors on (or composes over) an umbrella tag; the registry-published
/// versions are read only for a registry-anchored source. Both are handed to
/// [`toven_version::resolve_baseline`], which owns the anchor policy.
///
/// # Errors
/// Propagates a typed VCS failure from reading a module's manifest at the
/// umbrella tag commit, or a manifest-parse failure from the adapter. Registry
/// failures downgrade to an empty published set rather than propagate.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn resolve_baseline(
    module: &Module,
    source: &BaselineSource,
    reader: &dyn VcsReader,
    version_source: &dyn VersionSource,
    tags: &[TagRef],
) -> AppResult<ReleaseBaseline> {
    let module_version_at_ref = umbrella_scheme(source)
        .and_then(|scheme| latest_matching(scheme, tags))
        .map(|(_, tag)| tag.target)
        .map(|commit| module_version_at(module, &commit, reader, version_source))
        .transpose()?
        .flatten();
    let published = if references_registry(source) {
        version_source
            .published_versions(module)
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    Ok(decide_baseline(
        &module.key(),
        source,
        tags,
        module_version_at_ref.as_ref(),
        &published,
    ))
}

/// The umbrella tag scheme a source anchors on (or composes a registry anchor
/// over), if any — the only source shape whose baseline version depends on the
/// module's own version at a tag commit.
fn umbrella_scheme(source: &BaselineSource) -> Option<&TagScheme> {
    match source {
        BaselineSource::UmbrellaTag { umbrella_scheme } => Some(umbrella_scheme),
        BaselineSource::Registry { diff } => umbrella_scheme(diff),
        _ => None,
    }
}

/// Whether the source anchors idempotency on the registry's published versions.
const fn references_registry(source: &BaselineSource) -> bool {
    matches!(source, BaselineSource::Registry { .. })
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

#[cfg(test)]
mod tests {
    use rskit_version::semver::Version;
    use toven_model::{EcosystemId, Module, ModuleRef, RepoPath};
    use toven_ports::{Oid, TagRef, TagScheme};
    use toven_testkit::{FakeReleaseTarget, FakeVcsReader};

    use super::resolve_baseline;
    use crate::BaselineSource;

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
}
