//! Immutable release planning vocabulary.

use rskit_version::semver::Version;
use toven_model::ModuleKey;
use toven_ports::{BumpLevel, Oid, PublicationPolicy, ReleaseMutation, TagSigner, Visibility};

/// The engine-owned named bump policy.
///
/// The `[…release].strategy` config field resolves to one of these. It selects
/// only the **decide next version** node of the release flow; every other node
/// (change detection, cascade, idempotency, tag/publish) is common to all
/// policies. Prerelease behavior is driven by `--pre <channel>` / the
/// `prerelease` config under [`SemverCascade`](Self::SemverCascade); under
/// [`Manifest`](Self::Manifest) the channel lives in the declared version.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub enum BumpPolicy {
    /// Semantic-version cascade: compute the next version from baseline +
    /// changes — patch by default, minor on a breaking signal, major on
    /// explicit request, finalizing a pending prerelease on a stable bump and
    /// cascading a dependency-floor bump into dependents. Prerelease is driven
    /// only by `--pre`/config. The default.
    SemverCascade,
    /// Manifest-declared: cut exactly the version the manifest declares, when
    /// it is strictly ahead of the last release tag; fail closed otherwise. The
    /// prerelease channel, if any, is part of the declared version.
    Manifest,
}

impl BumpPolicy {
    /// Canonical policy name used by config and reports.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SemverCascade => "semver-cascade",
            Self::Manifest => "manifest",
        }
    }
}

/// Which input decided a module's bump, under the documented precedence (argv >
/// `[modules.<name>.release]` > `[ecosystems.<id>].release` > adapter default).
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub enum BumpSource {
    /// An explicit `--set-version <module>=<x.y.z>` argv override pinned the
    /// target version.
    SetVersion,
    /// An argv level override (`--patch`/`--minor`/`--major <module>`) forced
    /// the level.
    Argv,
    /// The resolved config level (`[modules.<name>.release]` or
    /// `[ecosystems.<id>].release`) selected the level.
    Config,
    /// `Auto` resolved to a minor bump from a breaking changelog
    /// classification.
    Changelog,
    /// `Auto` resolved to the patch default (no breaking signal).
    Default,
    /// A dependency-floor cascade into a dependent that did not itself change.
    Cascade,
    /// The `manifest` bump policy cut the version declared in the manifest.
    Manifest,
}

impl BumpSource {
    /// Canonical report name for the winning input.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SetVersion => "set-version",
            Self::Argv => "argv",
            Self::Config => "config",
            Self::Changelog => "changelog",
            Self::Default => "default",
            Self::Cascade => "cascade",
            Self::Manifest => "manifest",
        }
    }
}

/// Why a module receives a release bump.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub enum BumpReason {
    /// The module itself changed since its release baseline.
    Changed,
    /// The module has never been released, so its declared version is cut as
    /// its first release.
    InitialRelease,
    /// The module bumped only because a dependency's floor rose (cascade).
    DependencyCascade,
    /// The module was pinned to an explicit target version.
    Explicit,
    /// The `manifest` bump policy cut the version declared in the manifest.
    Manifest,
}

impl BumpReason {
    /// Canonical report name for the reason.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Changed => "changed",
            Self::InitialRelease => "initial-release",
            Self::DependencyCascade => "dependency-cascade",
            Self::Explicit => "explicit",
            Self::Manifest => "manifest",
        }
    }
}

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
    #[must_use]
    pub const fn is_initial(&self) -> bool {
        self.tag.is_none()
    }
}

/// Human- and machine-consumable changelog summary for a module release.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ChangelogEntry {
    /// Module the entry describes.
    pub module: ModuleKey,
    /// Short summary derived from changed paths/commits.
    pub summary: String,
    /// Detailed lines for later report rendering.
    pub lines: Vec<String>,
    /// Whether the change classification marks this release as breaking.
    pub breaking: bool,
}

impl ChangelogEntry {
    /// Construct a non-breaking changelog entry.
    #[must_use]
    pub fn new(module: ModuleKey, summary: impl Into<String>, lines: Vec<String>) -> Self {
        Self {
            module,
            summary: summary.into(),
            lines,
            breaking: false,
        }
    }

    /// Mark this entry as a breaking change.
    #[must_use]
    pub const fn with_breaking(mut self, breaking: bool) -> Self {
        self.breaking = breaking;
        self
    }
}

/// How a module's release refs reach the remote: nothing, the release
/// commit's branch plus its tags, or only the tags.
///
/// The `push`/`push_branch` settings pair resolves to this one policy so the
/// meaningless combination — pushing nothing, yet selecting a branch-push
/// behavior — is unrepresentable.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub enum PushPolicy {
    /// Push nothing; the release commit and tags remain local (`push = false`,
    /// or `--no-push` at apply time).
    Disabled,
    /// Push the release commit's branch alongside the release tags.
    BranchAndTags,
    /// Push only the release tags — the mode a protected release branch
    /// requires, where the release commit lands through a pull request.
    TagsOnly,
}

impl PushPolicy {
    /// Resolve the policy from the `push`/`push_branch` settings pair.
    #[must_use]
    pub const fn resolve(push: bool, push_branch: bool) -> Self {
        if !push {
            Self::Disabled
        } else if push_branch {
            Self::BranchAndTags
        } else {
            Self::TagsOnly
        }
    }

    /// Whether any release refs are pushed at all.
    #[must_use]
    pub const fn permits_push(self) -> bool {
        !matches!(self, Self::Disabled)
    }

    /// Whether the release commit's branch is pushed alongside the tags.
    #[must_use]
    pub const fn pushes_branch(self) -> bool {
        matches!(self, Self::BranchAndTags)
    }
}

/// One module's planned release mutation and publish decision.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ReleaseEntry {
    /// Module being considered for release.
    pub module: ModuleKey,
    /// Version declared by the adapter.
    pub current_version: Version,
    /// Version to release, if this module receives an own-version bump.
    pub planned_version: Option<Version>,
    /// The release tag a mutating run would create for the planned version,
    /// resolved through the target-owned tag scheme; `None` when the module
    /// receives no own-version bump (a dependency-floor-only entry).
    pub planned_tag: Option<String>,
    /// The effective bump level applied to reach the planned version.
    pub level: BumpLevel,
    /// Why this module is being bumped.
    pub reason: BumpReason,
    /// Which input won under the documented precedence.
    pub winning_input: BumpSource,
    /// The changed module that triggered this cascade, when `reason` is a
    /// dependency cascade.
    pub cascade_origin: Option<ModuleKey>,
    /// Prerelease channel applied to the planned version, when cutting a
    /// prerelease.
    pub prerelease_channel: Option<String>,
    /// Whether the planned version is already at/above the registry (or,
    /// offline, the release tag), making a real publish a reported no-op.
    pub up_to_date: bool,
    /// Atomic mutation to pass back to the ecosystem release target.
    pub mutation: ReleaseMutation,
    /// Typed publication policy: registry publication or tag-only. (Excluded
    /// modules never produce a plan entry.)
    pub publication: PublicationPolicy,
    /// Whether the publish loop must publish this module/version.
    pub publish_needed: bool,
    /// Configured tag-format override used to build the target-owned tag
    /// scheme.
    pub tag_format: Option<String>,
    /// Configured annotation template; `None` creates a lightweight tag.
    pub tag_message: Option<String>,
    /// Signing material when the release tag is signed (always annotated);
    /// `None` creates an unsigned tag.
    pub signer: Option<TagSigner>,
    /// Configured member release-commit message template.
    pub commit_message: Option<String>,
    /// Name of the environment variable holding the registry publish token,
    /// resolved from this module's release settings; `None` uses the publishing
    /// toolchain's ambient credential. Carries the variable *name* only — never
    /// the secret — so a registry adapter reads it at the toolchain boundary.
    pub token_env: Option<String>,
    /// Exposure this module's release is cut with, resolved from its release
    /// settings and enforced fail-closed at the registry-publish boundary (a
    /// non-public release aimed at a public-only registry is rejected). The tag
    /// push and hosted forge Release follow the remote repository's own
    /// exposure, so visibility is recorded intent there. Defaults to public.
    pub visibility: Visibility,
    /// How this module's member release refs are pushed: the release commit's
    /// branch alongside the tags, only the tags (tag-only mode for a
    /// protected branch), or nothing.
    pub push: PushPolicy,
    /// Remote the member release refs target.
    pub remote: String,
    /// Branches on which this module permits a release; empty permits any.
    pub branches: Vec<String>,
    /// Topological rank used for deterministic publish ordering.
    pub topo_rank: usize,
    /// Baseline used for change detection.
    pub baseline: Option<ReleaseBaseline>,
    /// Changelog entry planned for this module.
    pub changelog: ChangelogEntry,
    /// Workspace-relative changelog file the `bump` phase rolls when
    /// `changelog_roll` is set (defaulted to `CHANGELOG.md`).
    pub changelog_path: String,
    /// Whether the `bump` phase finalizes this module's changelog by moving the
    /// documented `## [Unreleased]` body under a versioned heading.
    pub changelog_roll: bool,
}

/// Immutable release plan produced by the release PLAN tail.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ReleasePlan {
    /// Selected engine-owned bump policy.
    pub policy: BumpPolicy,
    /// Per-module entries, already sorted in deterministic publish order.
    pub entries: Vec<ReleaseEntry>,
}

impl ReleasePlan {
    /// Construct a release plan.
    #[must_use]
    pub const fn new(policy: BumpPolicy, entries: Vec<ReleaseEntry>) -> Self {
        Self { policy, entries }
    }

    /// Number of entries that require an actual publish attempt.
    #[must_use]
    pub fn publish_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.publish_needed)
            .count()
    }

    /// Whether the plan contains no module entries.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// One module's read-only release status: what it declares versus what the
/// registry already publishes and the release tag it last cut.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ReleaseModuleStatus {
    /// Module the status describes.
    pub module: ModuleKey,
    /// Typed publication policy resolved for this module.
    pub publication: PublicationPolicy,
    /// Version the module's manifest currently declares.
    pub declared_version: Version,
    /// Newest release tag cut for the module, if any.
    pub latest_tag: Option<String>,
    /// Forge the module cuts a hosted Release on (`[…release.host].forge`), if
    /// any. `None` = the module does not participate in the host phase (a pure
    /// registry library that contributes no hosted Release). In a mixed repo
    /// this is how the binary app is shown to host the release the libraries
    /// only contribute notes to.
    pub host_forge: Option<String>,
    /// Versions the registry reports as already published (best-effort).
    pub published_versions: Vec<Version>,
    /// Whether the declared version is already released. Online, the registry
    /// reports it among the published versions (tag-only modules never publish,
    /// so they always report `false` online); offline, where release tags
    /// anchor idempotency, the newest release tag is at/above it.
    pub is_published: bool,
}

/// A read-only projection of every releasable module's
/// declared/published/tagged state. Produced without mutating any manifest,
/// tag, or registry.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ReleaseStatus {
    /// Per-module status, sorted in module-key order.
    pub modules: Vec<ReleaseModuleStatus>,
}

impl ReleaseStatus {
    /// Construct a release status projection.
    #[must_use]
    pub const fn new(modules: Vec<ReleaseModuleStatus>) -> Self {
        Self { modules }
    }
}

/// The rehearsal verdict for one planned release: what a real publish loop
/// would do, decided without any registry mutation.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub enum PublishDecision {
    /// The registry lacks this version; a real run would publish it.
    WouldPublish,
    /// The registry already reports this version; a real run would skip it.
    AlreadyPublished,
    /// This module is tag-only and never publishes to a package registry.
    TagOnly,
}

impl PublishDecision {
    /// Canonical wire/report name for the decision.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WouldPublish => "would-publish",
            Self::AlreadyPublished => "already-published",
            Self::TagOnly => "tag-only",
        }
    }
}

/// One planned release's place in the rehearsed publish order and its
/// would-publish/already-published verdict.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct RehearsalVerdict {
    /// Module being rehearsed.
    pub module: ModuleKey,
    /// Typed publication policy resolved for this module.
    pub publication: PublicationPolicy,
    /// Version that would be published.
    pub version: Version,
    /// Whether a real run would publish this version or find it already
    /// published.
    pub decision: PublishDecision,
}

/// One hosted forge Release a real publish run would cut, rehearsed without any
/// forge mutation.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct HostRehearsal {
    /// Forge that would host the Release.
    pub forge: String,
    /// Release tag the hosted Release would be cut against.
    pub tag: String,
    /// Whether the Release would be a draft.
    pub draft: bool,
    /// Whether the Release would be marked as a prerelease.
    pub prerelease: bool,
    /// Rendered Release notes body (the commit-derived, grouped changelog) that
    /// a real run would post, previewed mutation-free.
    pub notes: String,
    /// Project-relative artifact paths that would be uploaded.
    pub assets: Vec<String>,
}

/// A read-only rehearsal of the release publish loop: the resolved publish
/// order and per-module verdicts, computed without mutating manifests, tags, or
/// the registry.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ReleaseRehearsal {
    /// Selected engine-owned bump policy.
    pub policy: BumpPolicy,
    /// Per-module verdicts in deterministic publish order.
    pub verdicts: Vec<RehearsalVerdict>,
    /// Hosted forge Releases that a real run would cut, in publish order.
    pub hosted: Vec<HostRehearsal>,
}

impl ReleaseRehearsal {
    /// Construct a rehearsal report.
    #[must_use]
    pub const fn new(
        policy: BumpPolicy,
        verdicts: Vec<RehearsalVerdict>,
        hosted: Vec<HostRehearsal>,
    ) -> Self {
        Self {
            policy,
            verdicts,
            hosted,
        }
    }
}

/// Release-specific APPLY counters.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct ReleaseStats {
    /// Modules included in the release plan.
    pub planned_modules: usize,
    /// Manifests mutated during APPLY.
    pub mutated_modules: usize,
    /// Artifacts packaged and verified.
    pub packaged_artifacts: usize,
    /// Release tags created.
    pub tagged_modules: usize,
    /// Versions successfully published.
    pub published_modules: usize,
    /// Versions skipped because the registry already had them.
    pub skipped_published_modules: usize,
    /// Rate-limit waits performed by the publish loop.
    pub rate_limited_waits: usize,
    /// Hosted forge Releases created or updated after publish.
    pub hosted_releases: usize,
    /// Whether APPLY resumed an already-tagged release: the git mutation phase
    /// (manifest mutation, commit, tag, push) was skipped because every planned
    /// tag already exists, leaving only the idempotent publish and
    /// hosted-release phases to complete.
    pub resumed: bool,
}

impl ReleaseStats {
    /// Create empty release stats for a plan with `planned_modules`.
    #[must_use]
    pub const fn new(planned_modules: usize) -> Self {
        Self {
            planned_modules,
            mutated_modules: 0,
            packaged_artifacts: 0,
            tagged_modules: 0,
            published_modules: 0,
            skipped_published_modules: 0,
            rate_limited_waits: 0,
            hosted_releases: 0,
            resumed: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use rskit_version::semver::Version;
    use toven_model::{EcosystemId, ModuleKey, ModuleRef};
    use toven_ports::{BumpLevel, ReleaseMutation, Visibility};

    use super::{
        BumpPolicy, BumpReason, BumpSource, ChangelogEntry, PushPolicy, ReleaseEntry, ReleasePlan,
        ReleaseStats,
    };

    fn module(name: &str) -> ModuleKey {
        ModuleKey::bare(ModuleRef::new(EcosystemId::new("rust").unwrap(), name).unwrap())
    }

    #[test]
    fn policy_name_is_stable() {
        assert_eq!(BumpPolicy::SemverCascade.as_str(), "semver-cascade");
        assert_eq!(BumpPolicy::Manifest.as_str(), "manifest");
    }

    #[test]
    fn bump_source_and_reason_names_are_stable() {
        assert_eq!(BumpSource::SetVersion.as_str(), "set-version");
        assert_eq!(BumpSource::Cascade.as_str(), "cascade");
        assert_eq!(BumpSource::Manifest.as_str(), "manifest");
        assert_eq!(BumpReason::Changed.as_str(), "changed");
        assert_eq!(BumpReason::DependencyCascade.as_str(), "dependency-cascade");
        assert_eq!(BumpReason::Manifest.as_str(), "manifest");
    }

    #[test]
    fn publish_count_counts_only_needed_entries() {
        let entry = |name: &str, publish_needed: bool| ReleaseEntry {
            module: module(name),
            current_version: Version::new(0, 1, 0),
            planned_version: Some(Version::new(0, 2, 0)),
            planned_tag: Some(format!("rust/{name}@0.2.0")),
            level: BumpLevel::Minor,
            reason: BumpReason::Changed,
            winning_input: BumpSource::Default,
            cascade_origin: None,
            prerelease_channel: None,
            up_to_date: false,
            mutation: ReleaseMutation::version(Version::new(0, 2, 0)),
            publication: if publish_needed {
                toven_ports::PublicationPolicy::Registry {
                    registry: "crates-io".into(),
                }
            } else {
                toven_ports::PublicationPolicy::TagOnly
            },
            publish_needed,
            tag_format: None,
            tag_message: None,
            signer: None,
            commit_message: None,
            token_env: None,
            visibility: Visibility::Public,
            push: PushPolicy::BranchAndTags,
            remote: "origin".into(),
            branches: Vec::new(),
            topo_rank: 0,
            baseline: None,
            changelog: ChangelogEntry::new(module(name), "changed", Vec::new()),
            changelog_path: "CHANGELOG.md".into(),
            changelog_roll: false,
        };

        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", true), entry("app", false)],
        );

        assert_eq!(plan.publish_count(), 1);
        assert!(!plan.is_empty());
    }

    #[test]
    fn stats_start_empty_for_plan_size() {
        let stats = ReleaseStats::new(3);
        assert_eq!(stats.planned_modules, 3);
        assert_eq!(stats.published_modules, 0);
    }
}
