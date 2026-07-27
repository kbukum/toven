//! Immutable release planning vocabulary.

use rskit_version::semver::Version;
use toven_model::ModuleKey;
use toven_ports::{BaselineSpec, BumpLevel, Oid, PublicationPolicy, ReleaseMutation};

/// The engine-owned named bump policy.
///
/// The bump surface is a single matrix, not a family of named strategies. The
/// `[…release].strategy` config field is kept as a named selector so additional
/// policies can be introduced later without a config break, but it currently
/// resolves to exactly one policy: prerelease behavior is driven only by `--pre
/// <channel>` / the `prerelease` config, never by a policy name.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub enum BumpPolicy {
    /// Semantic-version cascade: patch by default, minor on a breaking signal,
    /// major on explicit request, cascading a dependency-floor bump into
    /// dependents. Prerelease is driven only by `--pre`/config.
    SemverCascade,
}

impl BumpPolicy {
    /// Canonical policy name used by config and reports.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SemverCascade => "semver-cascade",
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
        }
    }
}

/// Why a module receives a release bump.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub enum BumpReason {
    /// The module itself changed since its release baseline.
    Changed,
    /// The module bumped only because a dependency's floor rose (cascade).
    DependencyCascade,
    /// The module was pinned to an explicit target version.
    Explicit,
}

impl BumpReason {
    /// Canonical report name for the reason.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Changed => "changed",
            Self::DependencyCascade => "dependency-cascade",
            Self::Explicit => "explicit",
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
    /// Fallback baseline spec used when no prior release tag is available.
    pub fallback: Option<BaselineSpec>,
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
            fallback: None,
        }
    }

    /// Construct a baseline from the configured fallback strategy.
    #[must_use]
    pub const fn fallback(module: ModuleKey, spec: BaselineSpec) -> Self {
        Self {
            module,
            tag: None,
            version: None,
            target: None,
            fallback: Some(spec),
        }
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
    /// Configured member release-commit message template.
    pub commit_message: Option<String>,
    /// Whether this module permits its member release refs to be pushed.
    pub push: bool,
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
        }
    }
}

#[cfg(test)]
mod tests {
    use rskit_version::semver::Version;
    use toven_model::{EcosystemId, ModuleKey, ModuleRef};
    use toven_ports::{BumpLevel, ReleaseMutation};

    use super::{
        BumpPolicy, BumpReason, BumpSource, ChangelogEntry, ReleaseEntry, ReleasePlan, ReleaseStats,
    };

    fn module(name: &str) -> ModuleKey {
        ModuleKey::bare(ModuleRef::new(EcosystemId::new("rust").unwrap(), name).unwrap())
    }

    #[test]
    fn policy_name_is_stable() {
        assert_eq!(BumpPolicy::SemverCascade.as_str(), "semver-cascade");
    }

    #[test]
    fn bump_source_and_reason_names_are_stable() {
        assert_eq!(BumpSource::SetVersion.as_str(), "set-version");
        assert_eq!(BumpSource::Cascade.as_str(), "cascade");
        assert_eq!(BumpReason::Changed.as_str(), "changed");
        assert_eq!(BumpReason::DependencyCascade.as_str(), "dependency-cascade");
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
            commit_message: None,
            push: true,
            remote: "origin".into(),
            branches: Vec::new(),
            topo_rank: 0,
            baseline: None,
            changelog: ChangelogEntry::new(module(name), "changed", Vec::new()),
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
