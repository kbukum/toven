//! Immutable plan artifacts produced by the PLAN half of the engine.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::capability::Capability;
use crate::identity::{ModuleKey, RepoPath, WorkspaceId};

/// Per-unit cache outcome decided statically during PLAN.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheVerdict {
    /// A reusable cache record exists; the unit is skipped at exec.
    Hit,
    /// No usable record; the unit must run.
    Miss,
    /// Caching is disabled for this unit; it always runs.
    Disabled,
    /// Cache read skipped due to force mode; the unit runs and rewrites.
    Forced,
}

impl CacheVerdict {
    /// Whether this verdict skips execution (only a [`CacheVerdict::Hit`]).
    #[must_use]
    pub const fn is_hit(self) -> bool {
        matches!(self, Self::Hit)
    }
}

/// How a persistent execution unit reports readiness.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionReadiness {
    /// Ready once the subprocess starts.
    Started,
    /// Ready when a bounded health command exits successfully.
    Command(Vec<String>),
    /// Ready when literal text appears on stdout/stderr.
    OutputContains(String),
}

/// Provenance of a resolved task — where its command shape came from.
///
/// Populated on each [`ExecutionUnit`] so plan output and reports can explain
/// which config layer won during field-merge. The same vocabulary is
/// re-exported by `toven-ports` and carried on the adapter-facing `Task`, so
/// the ports layer and the plan artifact speak one origin type.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum TaskOrigin {
    /// The adapter's built-in default for the kind.
    #[default]
    AdapterDefault,
    /// A project-level `[ecosystems.<id>.tasks.<name>]` override.
    Project,
    /// A group-level `[groups.<name>.tasks.<name>]` override that layers on top
    /// of the ecosystem/adapter default for that group's members only.
    Group,
}

impl TaskOrigin {
    /// The stable kebab-case label for reporting (`adapter-default`, `project`,
    /// `group`), matching the serialized representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AdapterDefault => "adapter-default",
            Self::Project => "project",
            Self::Group => "group",
        }
    }
}

/// One schedulable unit of work in a [`Plan`].
///
/// A unit carries the rendered invocation plus the planning facts the APPLY
/// half and the report need. It is vocabulary: the engine populates it,
/// adapters and reporters read it.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
pub struct ExecutionUnit {
    /// Stable unit identifier (referenced by the wave order and events).
    pub id: String,
    /// Module this unit operates on (the representative module for a batched or
    /// whole-workspace unit). See [`members`](Self::members) for the full set.
    pub module: ModuleKey,
    /// Every module this unit covers. A `PerModule` unit lists exactly its one
    /// module; a `Batchable`/`WholeWorkspace` unit lists every module collapsed
    /// into the single invocation. Always non-empty and contains `module`.
    #[serde(default)]
    pub members: Vec<ModuleKey>,
    /// Name of the task this unit runs — its identity (the config table key,
    /// e.g. `build`, `test`, `my-test`), not its recognized kind.
    pub task: String,
    /// Provenance of the task this unit runs (which config layer won).
    #[serde(default)]
    pub origin: TaskOrigin,
    /// Owning workspace, whose toolchain identity keys the cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceId>,
    /// Fully rendered command line.
    pub argv: Vec<String>,
    /// Whether this unit starts a long-lived (persistent) process.
    #[serde(default)]
    pub persistent: bool,
    /// Readiness policy for persistent units.
    #[serde(default = "default_readiness")]
    pub readiness: ExecutionReadiness,
    /// Bound on how long to wait for persistent readiness.
    #[serde(default = "default_readiness_timeout")]
    pub readiness_timeout: Duration,
    /// Whether any stdout output turns a zero-exit run into a failure. Set for
    /// a list-mode verification (e.g. `gofmt -l`) whose tool reports offenders
    /// on stdout but still exits `0`, so APPLY treats a non-empty stdout as a
    /// gate failure.
    #[serde(default)]
    pub fail_if_output: bool,
    /// Cache outcome decided during PLAN.
    pub cache: CacheVerdict,
    /// Content cache key to record after a successful cacheable run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_key: Option<String>,
    /// Declared outputs, repo-relative. A cache hit counts only while they
    /// are present and unchanged; APPLY records their state after success.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<RepoPath>,
    /// Ids of the units this unit depends on (the scheduled dependency edges).
    ///
    /// APPLY uses these to fail-close: when a unit fails, every unit that lists
    /// it (transitively) is marked [`Blocked`](crate::UnitStatus::Blocked) and
    /// never runs. Empty for a leaf unit.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    /// Optional within-wave serialization key (a shared resource such as a
    /// build target directory). Units sharing a `resource_group` run serially;
    /// units in different groups (or with none) may run in parallel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_group: Option<String>,
}

const fn default_readiness() -> ExecutionReadiness {
    ExecutionReadiness::Started
}

const fn default_readiness_timeout() -> Duration {
    Duration::from_secs(30)
}

/// Why an active module is not eligible for a task.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
#[non_exhaustive]
pub enum SkipReason {
    /// The task's `modules` list does not name the module.
    NotListed,
    /// The module lacks capabilities the task `requires` (a package with no
    /// `test` script under `toven test`).
    MissingCapabilities {
        /// The required capabilities the module does not provide.
        missing: Vec<Capability>,
    },
}

impl SkipReason {
    /// The stable kebab-case code (`not-listed`, `missing-capabilities`),
    /// matching the serialized `kind`.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotListed => "not-listed",
            Self::MissingCapabilities { .. } => "missing-capabilities",
        }
    }

    /// The missing capabilities, empty for a reason that has none.
    #[must_use]
    pub fn missing(&self) -> &[Capability] {
        match self {
            Self::NotListed => &[],
            Self::MissingCapabilities { missing } => missing,
        }
    }
}

/// An active module the plan left out because it is not eligible for the task.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
pub struct SkippedModule {
    /// The skipped module.
    pub module: ModuleKey,
    /// The task it was skipped for.
    pub task: String,
    /// Why the module is not eligible.
    pub reason: SkipReason,
}

/// The immutable result of the PLAN half: units + federated wave order.
///
/// `waves` lists [`ExecutionUnit::id`]s in dependency-respecting order; each
/// inner vector is one ready wave. The plan fully determines APPLY (including
/// which units are cache hits), so `--explain`/dry-run is just a PLAN-only
/// projection. `skipped` records active modules that are not eligible for the
/// task, so a skip is always explained rather than silent.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
pub struct Plan {
    /// All execution units, keyed by `id`.
    pub units: Vec<ExecutionUnit>,
    /// Wave-ordered unit ids (each inner vec is one ready wave).
    pub waves: Vec<Vec<String>>,
    /// Active modules left out for missing required capabilities.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<SkippedModule>,
}

impl Plan {
    /// Construct a plan from its units and wave order, with no skipped
    /// modules.
    #[must_use]
    pub const fn new(units: Vec<ExecutionUnit>, waves: Vec<Vec<String>>) -> Self {
        Self {
            units,
            waves,
            skipped: Vec::new(),
        }
    }

    /// Record the active modules the plan left out.
    #[must_use]
    pub fn with_skipped(mut self, skipped: Vec<SkippedModule>) -> Self {
        self.skipped = skipped;
        self
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{CacheVerdict, ExecutionReadiness, ExecutionUnit, Plan, SkipReason, SkippedModule};
    use crate::capability::Capability;
    use crate::identity::{EcosystemId, ModuleKey, ModuleRef};

    #[test]
    fn plan_serde_round_trip() {
        let unit = ExecutionUnit {
            id: "rust:errors#build".to_string(),
            module: ModuleKey::bare(
                ModuleRef::new(EcosystemId::new("rust").unwrap(), "errors").unwrap(),
            ),
            members: vec![ModuleKey::bare(
                ModuleRef::new(EcosystemId::new("rust").unwrap(), "errors").unwrap(),
            )],
            task: "build".to_string(),
            origin: super::TaskOrigin::Group,
            workspace: None,
            argv: vec!["cargo".to_string(), "build".to_string()],
            persistent: false,
            readiness: ExecutionReadiness::Started,
            readiness_timeout: Duration::from_secs(30),
            fail_if_output: false,
            cache: CacheVerdict::Miss,
            cache_key: Some("cache-key".to_string()),
            outputs: Vec::new(),
            depends_on: vec!["rust:core#build".to_string()],
            resource_group: Some("cargo:.".to_string()),
        };
        let plan =
            Plan::new(vec![unit], vec![vec!["rust:errors#build".to_string()]]).with_skipped(vec![
                SkippedModule {
                    module: ModuleKey::bare(
                        ModuleRef::new(EcosystemId::new("rust").unwrap(), "macros").unwrap(),
                    ),
                    task: "build".to_string(),
                    reason: SkipReason::MissingCapabilities {
                        missing: vec![Capability::new("bin").unwrap()],
                    },
                },
            ]);
        let json = serde_json::to_string(&plan).unwrap();
        let back: Plan = serde_json::from_str(&json).unwrap();
        assert_eq!(plan, back);
        assert!(
            json.contains(r#""reason":{"kind":"missing-capabilities","missing":["bin"]}"#),
            "{json}"
        );
        assert_eq!(plan.skipped[0].reason.code(), "missing-capabilities");
        assert!(SkipReason::NotListed.missing().is_empty());
        assert!(!plan.units[0].cache.is_hit());
        assert_eq!(
            plan.units[0].depends_on,
            vec!["rust:core#build".to_string()]
        );
    }
}
