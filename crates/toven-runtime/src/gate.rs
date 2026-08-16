//! [`Gate`] — fail-closed reverse-dependency gating.
//!
//! When a unit fails, every unit that transitively depends on it is blocked; a
//! failed unit never blocks its own dependencies. This is the same gating the
//! `run` APPLY walk relies on, lifted here so every verb shares one implementation.

use std::collections::{BTreeMap, BTreeSet};

use crate::graph::UnitSpec;

/// Runtime scheduling state for one unit.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum UnitState {
    /// Not yet settled.
    Pending,
    /// The unit completed without failing.
    Satisfied,
    /// The unit failed.
    Failed,
    /// The unit was blocked by an upstream failure.
    Blocked,
}

/// Reverse-dependency gate used to block dependents after a failure.
pub struct Gate {
    reverse: BTreeMap<String, Vec<String>>,
    states: BTreeMap<String, UnitState>,
}

impl Gate {
    /// Build a gate for `units`, seeding every unit `Pending`.
    #[must_use]
    pub fn new(units: &[UnitSpec]) -> Self {
        let mut reverse: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut states = BTreeMap::new();
        for unit in units {
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
    /// than [`UnitState::Blocked`]: an unknown id signals an internal
    /// inconsistency the caller must surface loudly, not silently skip as if it
    /// were upstream-blocked.
    #[must_use]
    pub fn state(&self, unit_id: &str) -> UnitState {
        self.states
            .get(unit_id)
            .copied()
            .unwrap_or(UnitState::Pending)
    }

    /// Mark a unit as satisfied (completed without failing).
    pub fn satisfy(&mut self, unit_id: &str) {
        self.states
            .insert(unit_id.to_string(), UnitState::Satisfied);
    }

    /// Mark a unit as failed and return each reverse-dependent it blocks.
    ///
    /// Only `Pending` dependents transition to `Blocked` (an already-settled
    /// dependent keeps its state), and the walk is de-duplicated so a diamond
    /// dependency blocks each node once. The returned ids are exactly those to
    /// emit a terminal `Blocked` outcome for.
    pub fn fail_and_block_dependents(&mut self, unit_id: &str) -> Vec<String> {
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

#[cfg(test)]
mod tests {
    use super::{Gate, UnitState};
    use crate::graph::UnitSpec;

    fn spec(id: &str, deps: &[&str]) -> UnitSpec {
        UnitSpec::new(id, deps.iter().copied())
    }

    #[test]
    fn failure_blocks_only_transitive_dependents() {
        // root <- mid <- leaf, and an independent sibling off root.
        let units = [
            spec("root", &[]),
            spec("mid", &["root"]),
            spec("leaf", &["mid"]),
            spec("sibling", &["root"]),
        ];
        let mut gate = Gate::new(&units);

        let blocked = gate.fail_and_block_dependents("mid");
        assert_eq!(blocked, vec!["leaf".to_string()]);
        assert_eq!(gate.state("mid"), UnitState::Failed);
        assert_eq!(gate.state("leaf"), UnitState::Blocked);
        // The dependency and the unrelated sibling are untouched.
        assert_eq!(gate.state("root"), UnitState::Pending);
        assert_eq!(gate.state("sibling"), UnitState::Pending);
    }

    #[test]
    fn diamond_dependent_is_blocked_once() {
        // top <- {left, right} <- bottom.
        let units = [
            spec("top", &[]),
            spec("left", &["top"]),
            spec("right", &["top"]),
            spec("bottom", &["left", "right"]),
        ];
        let mut gate = Gate::new(&units);

        let mut blocked = gate.fail_and_block_dependents("top");
        blocked.sort();
        assert_eq!(blocked, vec!["bottom", "left", "right"]);
    }

    #[test]
    fn an_already_satisfied_dependent_is_not_reblocked() {
        let units = [spec("root", &[]), spec("dep", &["root"])];
        let mut gate = Gate::new(&units);
        gate.satisfy("dep");
        assert!(gate.fail_and_block_dependents("root").is_empty());
        assert_eq!(gate.state("dep"), UnitState::Satisfied);
    }
}
