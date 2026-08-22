//! Compute-budget sizing for CPU-bound tool fan-out.
//!
//! A per-module (`PerModule`) task fans out into one child process per module,
//! and the worker pool runs several at once. Left unbounded, each child also
//! defaults its own internal parallelism to the whole machine, so peak thread
//! pressure approaches cores². This resolves a total thread budget `B` and
//! divides it across the units running concurrently in a wave, handing each
//! fanned-out tool its share through an environment variable (never argv). A
//! self-balancing single-invocation toolchain (one `cargo` build) registers no
//! env name, so nothing is injected and it runs with its own default
//! parallelism, unaffected by the budget.
//!
//! The total budget is per-ecosystem: an `[ecosystems.<id>].compute_budget`
//! override wins over the global `[toven].compute_budget`, so a polyglot repo
//! can size Go and (say) a future fan-out ecosystem independently.

use std::collections::BTreeMap;

use toven_model::EcosystemId;
use toven_ports::ComputeBudget;

/// Per-process floor: never hand a fanned-out tool fewer than this many threads,
/// so a saturated wave never starves each child down to a single thread (which
/// measured far slower than the machine's core count).
const PER_PROCESS_FLOOR: usize = 2;

/// Resolved compute-budget policy for one APPLY run.
///
/// Holds the global [`ComputeBudget`], its per-ecosystem overrides, and the
/// per-ecosystem environment-variable names each fanned-out tool's share is
/// injected through.
#[derive(Debug, Clone)]
pub(super) struct BudgetPlan {
    /// Default budget for any ecosystem without an explicit override.
    global: ComputeBudget,
    /// Per-ecosystem budget overrides (`[ecosystems.<id>].compute_budget`).
    overrides: BTreeMap<EcosystemId, ComputeBudget>,
    /// Environment-variable names each ecosystem's tools read for their share.
    env_names: BTreeMap<EcosystemId, Vec<String>>,
}

impl BudgetPlan {
    /// Assemble a policy from the global budget, its per-ecosystem overrides,
    /// and the ecosystem→env-name map the CLI built from the configured
    /// adapters.
    pub(super) const fn new(
        global: ComputeBudget,
        overrides: BTreeMap<EcosystemId, ComputeBudget>,
        env_names: BTreeMap<EcosystemId, Vec<String>>,
    ) -> Self {
        Self {
            global,
            overrides,
            env_names,
        }
    }

    /// Whether any ecosystem can be injected, so the wave loop can skip the
    /// per-unit computation entirely when none can.
    pub(super) fn is_active(&self) -> bool {
        self.env_names
            .iter()
            .any(|(ecosystem, names)| !names.is_empty() && self.total_for(ecosystem).is_some())
    }

    /// The per-process environment for a unit of `ecosystem` in a wave running
    /// `concurrent` units at once (already capped at `max_parallel`).
    ///
    /// Empty when the ecosystem's budget is opted out, it registers no name, or
    /// the registered list is empty — i.e. the unit runs with the tool's own
    /// default parallelism.
    pub(super) fn env_for(
        &self,
        ecosystem: &EcosystemId,
        concurrent: usize,
    ) -> BTreeMap<String, String> {
        let Some(names) = self
            .env_names
            .get(ecosystem)
            .filter(|names| !names.is_empty())
        else {
            return BTreeMap::new();
        };
        let Some(total) = self.total_for(ecosystem) else {
            return BTreeMap::new();
        };
        let share = per_process(total, concurrent).to_string();
        names
            .iter()
            .map(|name| (name.clone(), share.clone()))
            .collect()
    }

    /// The effective [`ComputeBudget`] for `ecosystem` (override else global).
    fn budget_for(&self, ecosystem: &EcosystemId) -> ComputeBudget {
        self.overrides
            .get(ecosystem)
            .copied()
            .unwrap_or(self.global)
    }

    /// The resolved total thread budget for `ecosystem`, or `None` when opted
    /// out ([`ComputeBudget::Inherit`]).
    fn total_for(&self, ecosystem: &EcosystemId) -> Option<usize> {
        match self.budget_for(ecosystem) {
            ComputeBudget::Inherit => None,
            ComputeBudget::Fixed(threads) => Some(threads.get()),
            // `Auto` and any future sizing mode fall back to the host-sized,
            // load-agnostic budget.
            _ => Some(host_cpus()),
        }
    }
}

/// `clamp(ceil(total / concurrent), floor, total)` — each concurrent unit's
/// share of the total budget, never below the floor nor above the whole budget.
fn per_process(total: usize, concurrent: usize) -> usize {
    let divisor = concurrent.max(1);
    let share = total.div_ceil(divisor);
    // When the whole budget is below the floor, the budget itself is the ceiling
    // — clamp the floor down so the range stays valid (lo <= hi).
    let floor = PER_PROCESS_FLOOR.min(total);
    share.clamp(floor, total)
}

/// The host's usable CPU count, defaulting to 1 when the platform cannot report
/// it (the load-agnostic, deterministic `auto` budget).
fn host_cpus() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use toven_model::EcosystemId;
    use toven_ports::ComputeBudget;

    use super::{BudgetPlan, per_process};

    fn go() -> EcosystemId {
        EcosystemId::new("go").expect("valid id")
    }

    fn rust() -> EcosystemId {
        EcosystemId::new("rust").expect("valid id")
    }

    fn go_env() -> BTreeMap<EcosystemId, Vec<String>> {
        BTreeMap::from([(go(), vec!["GOMAXPROCS".to_string()])])
    }

    fn plan(global: ComputeBudget) -> BudgetPlan {
        BudgetPlan::new(global, BTreeMap::new(), go_env())
    }

    #[test]
    fn divides_the_budget_across_concurrent_units() {
        // 12 threads split across 4 concurrent units → 3 each.
        assert_eq!(per_process(12, 4), 3);
        // Ceiling division: 10 / 3 rounds up to 4.
        assert_eq!(per_process(10, 3), 4);
    }

    #[test]
    fn holds_the_per_process_floor() {
        // A saturated wave (12 units, 12 budget) would divide to 1, floored to 2.
        assert_eq!(per_process(12, 12), 2);
        assert_eq!(per_process(12, 100), 2);
    }

    #[test]
    fn never_exceeds_the_whole_budget() {
        // A single concurrent unit keeps the whole budget, not more.
        assert_eq!(per_process(8, 1), 8);
        // A tiny budget below the floor is its own ceiling (range stays valid).
        assert_eq!(per_process(1, 4), 1);
    }

    #[test]
    fn injects_the_registered_name_for_a_fanned_out_ecosystem() {
        let plan = plan(ComputeBudget::fixed(12));
        assert!(plan.is_active());
        let env = plan.env_for(&go(), 4);
        assert_eq!(env.get("GOMAXPROCS").map(String::as_str), Some("3"));
    }

    #[test]
    fn an_ecosystem_without_a_name_injects_nothing() {
        // Rust registers no env name (cargo self-balances) → no injection even
        // though Go does.
        let plan = plan(ComputeBudget::fixed(12));
        assert!(plan.env_for(&rust(), 4).is_empty());
    }

    #[test]
    fn inherit_opts_out_entirely() {
        let plan = plan(ComputeBudget::Inherit);
        assert!(!plan.is_active());
        assert!(plan.env_for(&go(), 4).is_empty());
    }

    #[test]
    fn no_registered_names_is_inactive() {
        let plan = BudgetPlan::new(ComputeBudget::Auto, BTreeMap::new(), BTreeMap::new());
        assert!(!plan.is_active());
    }

    #[test]
    fn a_per_ecosystem_override_wins_over_the_global_budget() {
        // Global inherits (opts out), but Go overrides to a fixed budget.
        let overrides = BTreeMap::from([(go(), ComputeBudget::fixed(8))]);
        let plan = BudgetPlan::new(ComputeBudget::Inherit, overrides, go_env());
        assert!(plan.is_active());
        assert_eq!(
            plan.env_for(&go(), 4).get("GOMAXPROCS").map(String::as_str),
            Some("2")
        );
    }

    #[test]
    fn a_per_ecosystem_inherit_override_opts_one_ecosystem_out() {
        // Global is fixed, but Go overrides to inherit → Go injects nothing.
        let overrides = BTreeMap::from([(go(), ComputeBudget::Inherit)]);
        let plan = BudgetPlan::new(ComputeBudget::fixed(12), overrides, go_env());
        assert!(!plan.is_active());
        assert!(plan.env_for(&go(), 4).is_empty());
    }
}
