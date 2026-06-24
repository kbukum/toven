//! Fail-closed dependency gating for APPLY.

use std::collections::{BTreeMap, BTreeSet};

use toven_model::{ExecutionUnit, Plan};

/// Runtime scheduling state for one unit.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum UnitState {
    /// Not yet satisfied or terminal.
    Pending,
    /// The unit succeeded, reached readiness, or was a cache hit.
    Satisfied,
    /// The unit failed.
    Failed,
    /// The unit was blocked by an upstream failure.
    Blocked,
}

/// Reverse-dependency gate used to block dependents after a failure.
pub(super) struct Gate {
    reverse: BTreeMap<String, Vec<String>>,
    states: BTreeMap<String, UnitState>,
}

impl Gate {
    /// Build a gate for `plan`.
    #[must_use]
    pub(super) fn new(plan: &Plan) -> Self {
        let mut reverse: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut states = BTreeMap::new();
        for unit in &plan.units {
            states.insert(unit.id.clone(), UnitState::Pending);
            for dependency in &unit.depends_on {
                reverse
                    .entry(dependency.clone())
                    .or_default()
                    .push(unit.id.clone());
            }
        }
        Self { reverse, states }
    }

    /// Current state for `unit_id`.
    ///
    /// An id the gate never indexed defaults to [`UnitState::Pending`] rather
    /// than [`UnitState::Blocked`]: an unknown id signals an internal plan
    /// inconsistency that must surface loudly (via the caller's `unit()` lookup)
    /// instead of being silently skipped as if it were upstream-blocked.
    pub(super) fn state(&self, unit_id: &str) -> UnitState {
        self.states
            .get(unit_id)
            .copied()
            .unwrap_or(UnitState::Pending)
    }

    /// Mark a unit as satisfied.
    pub(super) fn satisfy(&mut self, unit_id: &str) {
        self.states
            .insert(unit_id.to_string(), UnitState::Satisfied);
    }

    /// Mark a unit as failed and return every newly blocked reverse-dependent.
    pub(super) fn fail_and_block_dependents(&mut self, unit_id: &str) -> Vec<String> {
        self.states.insert(unit_id.to_string(), UnitState::Failed);
        let mut blocked = Vec::new();
        let mut seen = BTreeSet::new();
        let mut pending = self.reverse.get(unit_id).cloned().unwrap_or_default();
        while let Some(next) = pending.pop() {
            if !seen.insert(next.clone()) {
                continue;
            }
            if matches!(self.state(&next), UnitState::Pending) {
                self.states.insert(next.clone(), UnitState::Blocked);
                blocked.push(next.clone());
                pending.extend(self.reverse.get(&next).cloned().unwrap_or_default());
            }
        }
        blocked
    }
}

/// Index plan units by id.
pub(super) fn unit_index(plan: &Plan) -> BTreeMap<String, ExecutionUnit> {
    plan.units
        .iter()
        .map(|unit| (unit.id.clone(), unit.clone()))
        .collect()
}
