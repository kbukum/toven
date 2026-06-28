//! Immutable release planning vocabulary.

use rskit_version::semver::Version;
use toven_model::ModuleKey;
use toven_ports::{BaselineSpec, Oid, ReleaseMutation};

/// Engine-owned named release bump policies.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub enum ReleaseStrategyName {
    /// Cascade normal semantic-version bumps through affected dependents.
    SemverCascade,
    /// Keep prerelease/caret-compatible trains together.
    CaretPrerelease,
}

impl ReleaseStrategyName {
    /// Canonical strategy name used by config and reports.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SemverCascade => "semver-cascade",
            Self::CaretPrerelease => "caret-prerelease",
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
    /// Object id the release tag points at, when a prior release tag exists.
    pub target: Option<Oid>,
    /// Fallback baseline spec used when no prior release tag is available.
    pub fallback: Option<BaselineSpec>,
}

impl ReleaseBaseline {
    /// Construct a baseline from an existing module release tag.
    #[must_use]
    pub fn tag(module: ModuleKey, tag: impl Into<String>, target: Oid) -> Self {
        Self {
            module,
            tag: Some(tag.into()),
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
}

impl ChangelogEntry {
    /// Construct a changelog entry.
    #[must_use]
    pub fn new(module: ModuleKey, summary: impl Into<String>, lines: Vec<String>) -> Self {
        Self {
            module,
            summary: summary.into(),
            lines,
        }
    }
}

/// One module's planned release mutation and publish decision.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ReleaseEntry {
    /// Module being considered for release.
    pub module: ModuleKey,
    /// Version currently declared by the adapter.
    pub current_version: Version,
    /// Version to release, if this module receives an own-version bump.
    pub planned_version: Option<Version>,
    /// Atomic mutation to pass back to the ecosystem release target.
    pub mutation: ReleaseMutation,
    /// Whether the publish loop must publish this module/version.
    pub publish_needed: bool,
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
    /// Selected engine-owned release strategy.
    pub strategy: ReleaseStrategyName,
    /// Per-module entries, already sorted in deterministic publish order.
    pub entries: Vec<ReleaseEntry>,
}

impl ReleasePlan {
    /// Construct a release plan.
    #[must_use]
    pub const fn new(strategy: ReleaseStrategyName, entries: Vec<ReleaseEntry>) -> Self {
        Self { strategy, entries }
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
        }
    }
}

#[cfg(test)]
mod tests {
    use rskit_version::semver::Version;
    use toven_model::{EcosystemId, ModuleKey, ModuleRef};
    use toven_ports::ReleaseMutation;

    use super::{ChangelogEntry, ReleaseEntry, ReleasePlan, ReleaseStats, ReleaseStrategyName};

    fn module(name: &str) -> ModuleKey {
        ModuleKey::bare(ModuleRef::new(EcosystemId::new("rust").unwrap(), name).unwrap())
    }

    #[test]
    fn strategy_names_are_stable() {
        assert_eq!(
            ReleaseStrategyName::SemverCascade.as_str(),
            "semver-cascade"
        );
        assert_eq!(
            ReleaseStrategyName::CaretPrerelease.as_str(),
            "caret-prerelease"
        );
    }

    #[test]
    fn publish_count_counts_only_needed_entries() {
        let entry = |name: &str, publish_needed: bool| ReleaseEntry {
            module: module(name),
            current_version: Version::new(0, 1, 0),
            planned_version: Some(Version::new(0, 2, 0)),
            mutation: ReleaseMutation::version(Version::new(0, 2, 0)),
            publish_needed,
            topo_rank: 0,
            baseline: None,
            changelog: ChangelogEntry::new(module(name), "changed", Vec::new()),
        };

        let plan = ReleasePlan::new(
            ReleaseStrategyName::SemverCascade,
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
