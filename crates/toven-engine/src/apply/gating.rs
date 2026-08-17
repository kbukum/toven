//! Fail-closed dependency gating for APPLY.
//!
//! The gate itself is the generic [`toven_runtime::Gate`] — the one shared
//! reverse-dependency blocker every multi-module verb uses. This module only
//! adapts a [`Plan`] into that gate and keeps the APPLY-specific unit index.

use std::collections::BTreeMap;

use toven_model::{ExecutionUnit, Plan};
use toven_runtime::UnitSpec;

pub(super) use toven_runtime::{Gate, UnitState};

/// Build a fail-closed [`Gate`] for `plan` from its units' dependency edges.
#[must_use]
pub(super) fn gate_for(plan: &Plan) -> Gate {
    let specs: Vec<UnitSpec> = plan
        .units
        .iter()
        .map(|unit| UnitSpec::new(unit.id.clone(), unit.depends_on.clone()))
        .collect();
    Gate::new(&specs)
}

/// Index plan units by id.
pub(super) fn unit_index(plan: &Plan) -> BTreeMap<String, ExecutionUnit> {
    plan.units
        .iter()
        .map(|unit| (unit.id.clone(), unit.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use toven_runtime::{Gate, UnitSpec, UnitState};

    fn spec(id: &str, depends_on: &[&str]) -> UnitSpec {
        UnitSpec::new(id, depends_on.iter().copied())
    }

    #[test]
    fn failure_blocks_dependents_along_a_dag_without_mutual_blocking() {
        // The `rskit` facade-back-dep shape after layer-aware grouping: an acyclic base
        // → contrib → suite chain. Failing the middle unit blocks only its transitive
        // dependents; its dependency is never blocked back — the cyclic `core ⇄
        // contrib` mutual blocking is gone.
        let units = [
            spec("rust@core~~L0#check", &[]),
            spec("rust@contrib#check", &["rust@core~~L0#check"]),
            spec("rust@core~~L2#check", &["rust@contrib#check"]),
        ];
        let mut gate = Gate::new(&units);

        let blocked = gate.fail_and_block_dependents("rust@contrib#check");
        assert_eq!(blocked, vec!["rust@core~~L2#check".to_string()]);
        assert_eq!(gate.state("rust@contrib#check"), UnitState::Failed);
        assert_eq!(gate.state("rust@core~~L2#check"), UnitState::Blocked);
        // The base layer is a dependency of contrib, never a dependent: it stays
        // pending, proving no mutual blocking survives the DAG.
        assert_eq!(gate.state("rust@core~~L0#check"), UnitState::Pending);
    }
}
