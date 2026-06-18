//! Immutable plan artifacts produced by the PLAN half of the engine.

use serde::{Deserialize, Serialize};

use crate::identity::{ModuleRef, WorkspaceId};

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

/// One schedulable unit of work in a [`Plan`].
///
/// A unit carries the rendered invocation plus the planning facts the APPLY half
/// and the report need. It is vocabulary: the engine populates it, adapters and
/// reporters read it.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
pub struct ExecutionUnit {
    /// Stable unit identifier (referenced by the wave order and events).
    pub id: String,
    /// Module this unit operates on.
    pub module: ModuleRef,
    /// Task kind (e.g. `build`, `test`, `fmt`).
    pub kind: String,
    /// Owning workspace, whose toolchain identity keys the cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceId>,
    /// Fully rendered command line.
    pub argv: Vec<String>,
    /// Whether this unit starts a long-lived (persistent) process.
    #[serde(default)]
    pub persistent: bool,
    /// Cache outcome decided during PLAN.
    pub cache: CacheVerdict,
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
    use super::{CacheVerdict, ExecutionUnit, Plan};
    use crate::identity::{EcosystemId, ModuleRef};

    #[test]
    fn plan_serde_round_trip() {
        let unit = ExecutionUnit {
            id: "rust:errors#build".to_string(),
            module: ModuleRef::new(EcosystemId::new("rust").unwrap(), "errors").unwrap(),
            kind: "build".to_string(),
            workspace: None,
            argv: vec!["cargo".to_string(), "build".to_string()],
            persistent: false,
            cache: CacheVerdict::Miss,
        };
        let plan = Plan::new(vec![unit], vec![vec!["rust:errors#build".to_string()]]);
        let json = serde_json::to_string(&plan).unwrap();
        let back: Plan = serde_json::from_str(&json).unwrap();
        assert_eq!(plan, back);
        assert!(!plan.units[0].cache.is_hit());
    }
}
