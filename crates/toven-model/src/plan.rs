//! Immutable plan artifacts produced by the PLAN half of the engine.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::identity::{ModuleKey, WorkspaceId};

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
/// which config layer won during field-merge. The same vocabulary is re-exported
/// by `toven-ports` and carried on the adapter-facing `Task`, so the ports layer
/// and the plan artifact speak one origin type.
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
/// A unit carries the rendered invocation plus the planning facts the APPLY half
/// and the report need. It is vocabulary: the engine populates it, adapters and
/// reporters read it.
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
    /// Task kind (e.g. `build`, `test`, `fmt`).
    pub kind: String,
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
    /// Cache outcome decided during PLAN.
    pub cache: CacheVerdict,
    /// Content cache key to record after a successful cacheable run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_key: Option<String>,
    /// Ids of the units this unit depends on (the scheduled dependency edges).
    ///
    /// APPLY uses these to fail-close: when a unit fails, every unit that lists
    /// it (transitively) is marked [`Blocked`](crate::UnitStatus::Blocked) and
    /// never runs. Empty for a leaf unit.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    /// Optional within-wave serialization key (a shared resource such as a build
    /// target directory). Units sharing a `resource_group` run serially; units in
    /// different groups (or with none) may run in parallel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_group: Option<String>,
}

const fn default_readiness() -> ExecutionReadiness {
    ExecutionReadiness::Started
}

const fn default_readiness_timeout() -> Duration {
    Duration::from_secs(30)
}

/// The immutable result of the PLAN half: units + federated wave order.
///
/// `waves` lists [`ExecutionUnit::id`]s in dependency-respecting order; each inner
/// vector is one ready wave. The plan fully determines APPLY (including which
/// units are cache hits), so `--explain`/dry-run is just a PLAN-only projection.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
pub struct Plan {
    /// All execution units, keyed by `id`.
    pub units: Vec<ExecutionUnit>,
    /// Wave-ordered unit ids (each inner vec is one ready wave).
    pub waves: Vec<Vec<String>>,
}

impl Plan {
    /// Construct a plan from its units and wave order.
    #[must_use]
    pub const fn new(units: Vec<ExecutionUnit>, waves: Vec<Vec<String>>) -> Self {
        Self { units, waves }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{CacheVerdict, ExecutionReadiness, ExecutionUnit, Plan};
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
            kind: "build".to_string(),
            origin: super::TaskOrigin::Group,
            workspace: None,
            argv: vec!["cargo".to_string(), "build".to_string()],
            persistent: false,
            readiness: ExecutionReadiness::Started,
            readiness_timeout: Duration::from_secs(30),
            cache: CacheVerdict::Miss,
            cache_key: Some("cache-key".to_string()),
            depends_on: vec!["rust:core#build".to_string()],
            resource_group: Some("cargo:.".to_string()),
        };
        let plan = Plan::new(vec![unit], vec![vec!["rust:errors#build".to_string()]]);
        let json = serde_json::to_string(&plan).unwrap();
        let back: Plan = serde_json::from_str(&json).unwrap();
        assert_eq!(plan, back);
        assert!(!plan.units[0].cache.is_hit());
        assert_eq!(
            plan.units[0].depends_on,
            vec!["rust:core#build".to_string()]
        );
    }
}
