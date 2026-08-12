//! Release baseline anchoring: the [`BaselineSource`] policy and the **pure**
//! resolver that turns a selected source into a concrete [`ReleaseBaseline`].
//!
//! A module's release baseline answers "what changed **since the last
//! release**": it carries the version an idempotency check anchors on plus,
//! when a commit is available, the diff ref change detection compares files
//! against. The three-anchor policy — own tag, shared umbrella tag, registry
//! max — is named by [`BaselineSource`] and resolved by [`resolve_baseline`].
//!
//! The resolver is **pure**: every git/registry fact it needs is pre-gathered
//! (the matched `tags`, the module's own version at the umbrella commit, and the
//! registry's published versions) and passed in as data. All I/O happens in
//! GATHER, before the decision, so an umbrella workspace that versions its
//! modules independently under one shared tag anchors each module on its **own**
//! declared version at that tag's commit rather than on a re-parse in the engine.

use rskit_version::semver::Version;
use toven_model::ModuleKey;
use toven_ports::{Oid, TagRef, TagScheme};

use toven_semver::latest_matching;

/// The git baseline selected for one module's release change detection.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ReleaseBaseline {
    /// Module this baseline belongs to.
    pub module: ModuleKey,
    /// Tag name used as the baseline, when a prior release tag exists.
    pub tag: Option<String>,
    /// Version parsed from the baseline release tag, when one exists. Anchors
    /// offline idempotency (a planned version at/below this is up to date).
    pub version: Option<Version>,
    /// Object id the release tag points at, when a prior release tag exists.
    pub target: Option<Oid>,
}

impl ReleaseBaseline {
    /// Construct a baseline from an existing module release tag.
    #[must_use]
    pub fn tag(module: ModuleKey, tag: impl Into<String>, version: Version, target: Oid) -> Self {
        Self {
            module,
            tag: Some(tag.into()),
            version: Some(version),
            target: Some(target),
        }
    }

    /// Construct a baseline anchored on a registry or umbrella source.
    ///
    /// Unlike [`tag`](Self::tag) — a same-module release tag that supplies the
    /// tag name, version, and commit together — an anchored baseline may carry a
    /// version (the registry's max published version, or the version an umbrella
    /// tag denotes) and a diff ref (`tag`/`target`) that originate from
    /// *different* anchors, and either may be absent: a registry version with no
    /// tag commit to diff file changes against, or an umbrella tag whose commit
    /// anchors the diff. A baseline that carries at least one of a version or a
    /// diff ref is **not** an initial release — the module has a released anchor
    /// even without a tag of its own.
    #[must_use]
    pub const fn anchored(
        module: ModuleKey,
        tag: Option<String>,
        version: Option<Version>,
        target: Option<Oid>,
    ) -> Self {
        Self {
            module,
            tag,
            version,
            target,
        }
    }

    /// Construct the baseline of a module that has never been released.
    ///
    /// There is no prior release tag and no substitute ref to diff against:
    /// every source file is unreleased, so the module always joins the plan as
    /// an initial release.
    #[must_use]
    pub const fn initial(module: ModuleKey) -> Self {
        Self {
            module,
            tag: None,
            version: None,
            target: None,
        }
    }

    /// Whether this baseline marks a module that has never been released.
    ///
    /// A module is an initial release only when it has **no** anchor of any kind
    /// — no release tag, no anchored version, and no diff ref. A registry- or
    /// umbrella-anchored baseline carries a version and/or a diff ref, so it is
    /// change-gated against that anchor rather than treated as a first release.
    #[must_use]
    pub const fn is_initial(&self) -> bool {
        self.tag.is_none() && self.version.is_none() && self.target.is_none()
    }
}

/// Where a module's release change-detection baseline is anchored.
///
/// Resolving a source yields a [`ReleaseBaseline`] with a version (the
/// idempotency anchor) and, when a commit is available, a diff ref (the
/// file-diff anchor). The three variants mirror the two registry models Toven
/// supports — per-module tags *are* the registry (Go), while a registry
/// (crates.io) plus one umbrella tag describes a Rust workspace.
///
/// It is release *policy* and references [`TagScheme`], so it is owned here
/// beside the decision rather than leaking up into the reusable change
/// foundation.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum BaselineSource {
    /// The module's own latest release tag, matched by its own tag scheme — the
    /// per-module-tag model where a module carries independent release tags.
    OwnTag {
        /// The module's own tag scheme, matching tags like `rust/core@1.2.3`.
        scheme: TagScheme,
    },
    /// The latest tag matching a shared *umbrella* scheme (e.g. `v1.2.3`)
    /// supplies the diff ref (its commit), but the baseline version is the
    /// module's **own** version at that commit — falling back to the version the
    /// umbrella tag denotes only when the module's version there is absent. This
    /// is the workspace-shared model where modules release under one repo tag
    /// yet each keeps an independent version, so the shared tag is a diff anchor,
    /// not the per-module version authority.
    UmbrellaTag {
        /// The umbrella module's tag scheme, matching tags like `v1.2.3`.
        umbrella_scheme: TagScheme,
    },
    /// The registry's max published version anchors idempotency, while the diff
    /// ref is sourced from an inner tag anchor (`diff`). The effective baseline
    /// version is the **max** of the registry version and the `diff` anchor's
    /// version — the `max(registry max, version-at-tag)` composition a Rust
    /// workspace with crates.io history and one umbrella tag needs. A registry
    /// lookup failure (empty `published_versions`) downgrades to the `diff`
    /// anchor rather than aborting.
    Registry {
        /// The tag anchor supplying the diff ref (and a version to `max` with
        /// the registry version) — typically an [`OwnTag`](Self::OwnTag) or
        /// [`UmbrellaTag`](Self::UmbrellaTag).
        diff: Box<Self>,
    },
}

impl BaselineSource {
    /// Anchor on the module's own release tags.
    #[must_use]
    pub const fn own_tag(scheme: TagScheme) -> Self {
        Self::OwnTag { scheme }
    }

    /// Anchor on a shared umbrella tag scheme.
    #[must_use]
    pub const fn umbrella_tag(umbrella_scheme: TagScheme) -> Self {
        Self::UmbrellaTag { umbrella_scheme }
    }

    /// Anchor on the registry max published version, taking the diff ref from
    /// `diff` and the effective version from `max(registry, diff)`.
    #[must_use]
    pub fn registry(diff: Self) -> Self {
        Self::Registry {
            diff: Box::new(diff),
        }
    }
}

/// Resolve a module's release baseline from the selected [`BaselineSource`],
/// purely, over pre-gathered facts.
///
/// - [`OwnTag`](BaselineSource::OwnTag) selects the latest tag matching the
///   module's own scheme; the baseline carries that tag's version and commit.
/// - [`UmbrellaTag`](BaselineSource::UmbrellaTag) selects the latest tag
///   matching the shared umbrella scheme, but anchors the version on
///   `module_version_at_ref` — the module's **own** declared version at that
///   tag's commit, pre-read in GATHER (falling back to the umbrella tag's
///   version when the module has no resolvable version there).
/// - [`Registry`](BaselineSource::Registry) anchors the version on
///   `published_versions`' max, takes the diff ref from an inner tag anchor, and
///   uses the **max** of the two versions. An empty `published_versions` (a
///   registry lookup that failed or found nothing) downgrades to the inner tag
///   anchor and never aborts.
///
/// Either tag path is an initial release when no tag matches.
///
/// `module_version_at_ref` is the module's own version at the resolved umbrella
/// tag commit (`None` when the module has no manifest there or declares a
/// version the adapter cannot resolve). `published_versions` is the registry's
/// published set for the module (empty offline or on a lookup failure). Both are
/// gathered before the decision so this resolver stays pure and total.
#[must_use]
pub fn resolve_baseline(
    module: &ModuleKey,
    source: &BaselineSource,
    tags: &[TagRef],
    module_version_at_ref: Option<&Version>,
    published_versions: &[Version],
) -> ReleaseBaseline {
    match source {
        BaselineSource::OwnTag { scheme } => resolve_tag_anchor(module, scheme, tags),
        BaselineSource::UmbrellaTag {
            umbrella_scheme: scheme,
        } => resolve_umbrella_anchor(module, scheme, tags, module_version_at_ref),
        BaselineSource::Registry { diff } => resolve_registry(
            module,
            diff,
            tags,
            module_version_at_ref,
            published_versions,
        ),
    }
}

/// Resolve a tag-anchored baseline: the latest tag matching `scheme` supplies
/// the version (idempotency anchor) and its commit (diff ref); no matching tag
/// is an initial release.
fn resolve_tag_anchor(module: &ModuleKey, scheme: &TagScheme, tags: &[TagRef]) -> ReleaseBaseline {
    latest_matching(scheme, tags).map_or_else(
        || ReleaseBaseline::initial(module.clone()),
        |(version, tag)| ReleaseBaseline::tag(module.clone(), tag.name, version, tag.target),
    )
}

/// Resolve an umbrella-tag-anchored baseline: the latest tag matching the shared
/// umbrella `scheme` supplies the diff ref (its commit), but the version anchor
/// is the module's **own** declared version at that commit
/// (`module_version_at_ref`).
///
/// An umbrella workspace can version each module independently under one shared
/// tag, so the tag's own version is not the per-module release anchor. The
/// module's version at the tag commit is the authority; the umbrella tag's own
/// version is the fallback only when that version is absent (a module introduced
/// after the tag, or a manifest whose version the adapter cannot resolve). No
/// matching umbrella tag is an initial release.
fn resolve_umbrella_anchor(
    module: &ModuleKey,
    scheme: &TagScheme,
    tags: &[TagRef],
    module_version_at_ref: Option<&Version>,
) -> ReleaseBaseline {
    let Some((tag_version, tag)) = latest_matching(scheme, tags) else {
        return ReleaseBaseline::initial(module.clone());
    };
    let version = module_version_at_ref.cloned().unwrap_or(tag_version);
    ReleaseBaseline::tag(module.clone(), tag.name, version, tag.target)
}

/// Resolve a registry-anchored baseline: the registry's max published version
/// anchors idempotency, the diff ref comes from the inner tag anchor, and the
/// effective version is the max of the two. An empty `published_versions`
/// downgrades to the inner tag anchor rather than aborting — the publish loop's
/// `AlreadyPublished` classification remains the authoritative backstop, so a
/// transient registry outage must not fail change detection.
fn resolve_registry(
    module: &ModuleKey,
    diff: &BaselineSource,
    tags: &[TagRef],
    module_version_at_ref: Option<&Version>,
    published_versions: &[Version],
) -> ReleaseBaseline {
    let diff_baseline = resolve_baseline(
        module,
        diff,
        tags,
        module_version_at_ref,
        published_versions,
    );
    let registry_version = published_versions.iter().max().cloned();
    let version = max_version(registry_version, diff_baseline.version);
    ReleaseBaseline::anchored(
        module.clone(),
        diff_baseline.tag,
        version,
        diff_baseline.target,
    )
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
    use toven_model::{EcosystemId, ModuleRef};
    use toven_ports::{Oid, TagRef, TagScheme};

    use super::{BaselineSource, ReleaseBaseline, max_version, resolve_baseline};

    fn key(name: &str) -> toven_model::ModuleKey {
        ModuleRef::new(EcosystemId::new("rust").expect("ecosystem"), name)
            .expect("ref")
            .into()
    }

    fn tag(name: &str, oid: &str) -> TagRef {
        TagRef::new(name, Oid::new(oid))
    }

    fn resolve(source: &BaselineSource, tags: &[TagRef]) -> ReleaseBaseline {
        resolve_baseline(&key("core"), source, tags, None, &[])
    }

    #[test]
    fn own_tag_selects_the_latest_matching_tag() {
        let source = BaselineSource::own_tag(TagScheme::new("rust/core@", ""));
        let tags = vec![
            tag("rust/core@0.1.0", "a"),
            tag("rust/core@0.2.0", "b"),
            tag("rust/other@9.9.9", "c"),
        ];

        let baseline = resolve(&source, &tags);

        assert!(!baseline.is_initial());
        assert_eq!(baseline.version, Some(Version::new(0, 2, 0)));
        assert_eq!(baseline.tag.as_deref(), Some("rust/core@0.2.0"));
        assert_eq!(baseline.target.as_ref().map(Oid::as_str), Some("b"));
    }

    #[test]
    fn own_tag_without_a_matching_tag_is_an_initial_release() {
        let source = BaselineSource::own_tag(TagScheme::new("rust/core@", ""));
        let tags = vec![tag("rust/other@1.0.0", "a")];

        let baseline = resolve(&source, &tags);

        assert!(baseline.is_initial());
        assert_eq!(baseline.version, None);
        assert_eq!(baseline.target, None);
    }

    #[test]
    fn umbrella_tag_falls_back_to_the_tag_version_without_a_module_version() {
        // No module version at the umbrella commit: the baseline falls back to
        // the version the umbrella tag denotes.
        let source = BaselineSource::umbrella_tag(TagScheme::new("v", ""));
        let tags = vec![
            tag("rust/core@0.1.0", "own"),
            tag("v1.3.0", "umbrella-old"),
            tag("v1.4.0", "umbrella"),
        ];

        let baseline = resolve(&source, &tags);

        assert!(!baseline.is_initial());
        assert_eq!(baseline.version, Some(Version::new(1, 4, 0)));
        assert_eq!(baseline.tag.as_deref(), Some("v1.4.0"));
        assert_eq!(baseline.target.as_ref().map(Oid::as_str), Some("umbrella"));
    }

    #[test]
    fn umbrella_tag_anchors_on_the_module_version_at_the_tag_commit() {
        // The key independent-versioning case: the umbrella tag denotes 1.4.0,
        // but the module's own version at that commit (0.2.0) is the anchor —
        // the diff ref is still the umbrella tag commit.
        let source = BaselineSource::umbrella_tag(TagScheme::new("v", ""));
        let tags = vec![tag("v1.4.0", "umbrella")];
        let module_version = Version::new(0, 2, 0);

        let baseline = resolve_baseline(&key("core"), &source, &tags, Some(&module_version), &[]);

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
        let source =
            BaselineSource::registry(BaselineSource::umbrella_tag(TagScheme::new("v", "")));
        let tags = vec![tag("v1.4.0", "umbrella")];
        let module_version = Version::new(0, 2, 0);

        let baseline = resolve_baseline(
            &key("core"),
            &source,
            &tags,
            Some(&module_version),
            &[Version::new(0, 2, 0)],
        );

        assert_eq!(
            baseline.version,
            Some(Version::new(0, 2, 0)),
            "neither the registry nor the module version is the umbrella tag's 1.4.0"
        );
        assert_eq!(baseline.target.as_ref().map(Oid::as_str), Some("umbrella"));
    }

    #[test]
    fn registry_anchors_on_the_max_published_version() {
        let source =
            BaselineSource::registry(BaselineSource::umbrella_tag(TagScheme::new("v", "")));
        let tags = vec![tag("v1.1.0", "umbrella")];

        let baseline = resolve_baseline(
            &key("core"),
            &source,
            &tags,
            None,
            &[Version::new(1, 0, 0), Version::new(1, 2, 0)],
        );

        assert_eq!(baseline.version, Some(Version::new(1, 2, 0)));
        assert_eq!(baseline.target.as_ref().map(Oid::as_str), Some("umbrella"));
    }

    #[test]
    fn registry_takes_the_higher_of_registry_and_tag_versions() {
        let source =
            BaselineSource::registry(BaselineSource::umbrella_tag(TagScheme::new("v", "")));
        let tags = vec![tag("v1.3.0", "umbrella")];

        let baseline =
            resolve_baseline(&key("core"), &source, &tags, None, &[Version::new(1, 0, 0)]);

        assert_eq!(baseline.version, Some(Version::new(1, 3, 0)));
        assert_eq!(baseline.target.as_ref().map(Oid::as_str), Some("umbrella"));
    }

    #[test]
    fn registry_lookup_failure_downgrades_to_the_tag_anchor() {
        // An empty published set (a registry outage) must not abort: the
        // baseline downgrades to the diff tag anchor and still resolves.
        let source =
            BaselineSource::registry(BaselineSource::umbrella_tag(TagScheme::new("v", "")));
        let tags = vec![tag("v1.1.0", "umbrella")];

        let baseline = resolve_baseline(&key("core"), &source, &tags, None, &[]);

        assert_eq!(baseline.version, Some(Version::new(1, 1, 0)));
        assert_eq!(baseline.target.as_ref().map(Oid::as_str), Some("umbrella"));
        assert!(!baseline.is_initial());
    }

    #[test]
    fn registry_without_a_diff_tag_still_anchors_on_the_registry_version() {
        let source =
            BaselineSource::registry(BaselineSource::own_tag(TagScheme::new("rust/core@", "")));
        let tags = vec![tag("rust/other@1.0.0", "a")];

        let baseline =
            resolve_baseline(&key("core"), &source, &tags, None, &[Version::new(2, 0, 0)]);

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
