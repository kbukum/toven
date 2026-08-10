//! Release baseline resolution.
//!
//! Turns a [`BaselineSource`] policy choice into a concrete
//! [`ReleaseBaseline`] — the version an idempotency check anchors on plus, when
//! a commit is available, the diff ref change detection compares files against.
//! The resolver is the single home of the three-anchor policy (own tag,
//! umbrella tag, registry) so change detection (`change.rs`) selects a source
//! and delegates the mechanics here.
//!
//! Manifest parsing stays in the ecosystem adapters: an umbrella tag's baseline
//! version is the version that tag denotes (a workspace releases every module
//! together under one repo tag, so the umbrella tag's own version is the shared
//! version at its commit), never a manifest re-parse in the release engine.

use rskit_version::semver::Version;
use toven_model::Module;
use toven_ports::{TagRef, TagScheme, VersionSource};

use toven_core::vcs::latest_matching;

use crate::ReleaseBaseline;
use crate::model::BaselineSource;

/// Resolve a module's release baseline from the selected [`BaselineSource`].
///
/// - [`OwnTag`](BaselineSource::OwnTag) / [`UmbrellaTag`](BaselineSource::UmbrellaTag)
///   select the latest tag matching the given scheme; the baseline carries that
///   tag's version and commit, or is an initial release when no tag matches.
/// - [`Registry`](BaselineSource::Registry) anchors the version on the
///   registry's max published version, takes the diff ref from an inner tag
///   anchor, and uses the **max** of the two versions — the
///   `max(registry, version-at-tag)` composition. A registry lookup failure
///   downgrades to the inner tag anchor and never aborts.
///
/// # Errors
/// Currently infallible for every source (registry failures downgrade rather
/// than propagate), but returns [`AppResult`](rskit_errors::AppResult) so a
/// future source that must surface a typed VCS/registry failure can without a
/// signature change.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn resolve_baseline(
    module: &Module,
    source: &BaselineSource,
    version_source: &dyn VersionSource,
    tags: &[TagRef],
) -> rskit_errors::AppResult<ReleaseBaseline> {
    match source {
        BaselineSource::OwnTag { scheme }
        | BaselineSource::UmbrellaTag {
            umbrella_scheme: scheme,
        } => Ok(resolve_tag_anchor(module, scheme, tags)),
        BaselineSource::Registry { diff } => resolve_registry(module, diff, version_source, tags),
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

/// Resolve a registry-anchored baseline: the registry's max published version
/// anchors idempotency, the diff ref comes from the inner tag anchor, and the
/// effective version is the max of the two. A registry lookup failure downgrades
/// to the inner tag anchor rather than aborting — the publish loop's
/// `AlreadyPublished` classification remains the authoritative backstop, so a
/// transient registry outage must not fail change detection.
fn resolve_registry(
    module: &Module,
    diff: &BaselineSource,
    version_source: &dyn VersionSource,
    tags: &[TagRef],
) -> rskit_errors::AppResult<ReleaseBaseline> {
    let diff_baseline = resolve_baseline(module, diff, version_source, tags)?;
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
    use toven_testkit::FakeReleaseTarget;

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

        let baseline = resolve_baseline(&module, &source, &version_source, &tags).expect("resolve");

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

        let baseline = resolve_baseline(&module, &source, &version_source, &tags).expect("resolve");

        assert!(baseline.is_initial());
        assert_eq!(baseline.version, None);
        assert_eq!(baseline.target, None);
    }

    #[test]
    fn umbrella_tag_anchors_on_the_shared_umbrella_version() {
        // The module's own scheme (rust/core@) never matches; the umbrella scheme
        // (v) does, so the baseline is the version the umbrella tag denotes at its
        // commit — the workspace-shared version.
        let module = module("core", "crates/core");
        let source = BaselineSource::umbrella_tag(TagScheme::new("v", ""));
        let version_source = FakeReleaseTarget::new();
        let tags = vec![
            tag("rust/core@0.1.0", "own"),
            tag("v1.3.0", "umbrella-old"),
            tag("v1.4.0", "umbrella"),
        ];

        let baseline = resolve_baseline(&module, &source, &version_source, &tags).expect("resolve");

        assert!(!baseline.is_initial());
        assert_eq!(baseline.version, Some(Version::new(1, 4, 0)));
        assert_eq!(baseline.tag.as_deref(), Some("v1.4.0"));
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

        let baseline = resolve_baseline(&module, &source, &version_source, &tags).expect("resolve");

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

        let baseline = resolve_baseline(&module, &source, &version_source, &tags).expect("resolve");

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

        let baseline = resolve_baseline(&module, &source, &version_source, &tags).expect("resolve");

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

        let baseline = resolve_baseline(&module, &source, &version_source, &tags).expect("resolve");

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
