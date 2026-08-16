//! [`UnitSpec`] and [`level_waves`] — the generic unit graph and its
//! dependency-wave levelling.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use rskit_errors::{AppError, AppResult, ErrorCode};

/// One schedulable unit: a stable id and the ids it depends on.
///
/// The engine is graph-shape-agnostic — a unit is just an id plus its inbound
/// dependency edges. A verb lowers its modules/tasks to `UnitSpec`s; the engine
/// derives waves from the edges alone.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct UnitSpec {
    /// Stable, unique unit identifier.
    pub id: String,
    /// Ids this unit depends on; each must name another unit in the same set.
    pub depends_on: Vec<String>,
}

impl UnitSpec {
    /// Build a unit with the given id and dependency ids.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        depends_on: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            id: id.into(),
            depends_on: depends_on.into_iter().map(Into::into).collect(),
        }
    }
}

/// Level the units into dependency waves (Kahn layering).
///
/// Each returned wave holds ids whose dependencies all settled in an earlier
/// wave, so a wave's units are mutually independent and may run fully in
/// parallel. Ids within a wave are sorted for deterministic scheduling. An
/// edgeless set collapses to a single wave; an edged set produces one wave per
/// dependency level.
///
/// # Errors
/// Returns [`ErrorCode::InvalidInput`] if an id is duplicated, a `depends_on`
/// references an unknown unit, or the edges form a cycle (no valid ordering
/// exists).
pub fn level_waves(units: &[UnitSpec]) -> AppResult<Vec<Vec<String>>> {
    let mut indegree: BTreeMap<&str, usize> = BTreeMap::new();
    for unit in units {
        if indegree.insert(unit.id.as_str(), 0).is_some() {
            return Err(AppError::new(
                ErrorCode::InvalidInput,
                format!("duplicate unit id '{}'", unit.id),
            ));
        }
    }

    // Forward adjacency (dependency -> dependents) and indegree = #dependencies.
    let mut dependents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for unit in units {
        let mut seen = BTreeSet::new();
        for dep in &unit.depends_on {
            if !indegree.contains_key(dep.as_str()) {
                return Err(AppError::new(
                    ErrorCode::InvalidInput,
                    format!("unit '{}' depends on unknown unit '{dep}'", unit.id),
                ));
            }
            // A repeated edge must not inflate indegree, or the unit could never drain.
            if seen.insert(dep.as_str()) {
                dependents.entry(dep.as_str()).or_default().push(&unit.id);
                // `unit.id` was inserted into `indegree` above, so the entry exists.
                if let Some(degree) = indegree.get_mut(unit.id.as_str()) {
                    *degree += 1;
                }
            }
        }
    }

    let mut ready: VecDeque<&str> = indegree
        .iter()
        .filter(|&(_, &degree)| degree == 0)
        .map(|(&id, _)| id)
        .collect();
    let mut waves: Vec<Vec<String>> = Vec::new();
    let mut settled = 0usize;
    while !ready.is_empty() {
        let mut wave: Vec<&str> = ready.iter().copied().collect();
        wave.sort_unstable();
        ready.clear();
        for &id in &wave {
            settled += 1;
            for &dependent in dependents.get(id).map(Vec::as_slice).unwrap_or_default() {
                // Every dependent was inserted into `indegree` when its unit was indexed.
                if let Some(degree) = indegree.get_mut(dependent) {
                    *degree -= 1;
                    if *degree == 0 {
                        ready.push_back(dependent);
                    }
                }
            }
        }
        waves.push(wave.into_iter().map(str::to_string).collect());
    }

    if settled != units.len() {
        return Err(AppError::new(
            ErrorCode::InvalidInput,
            "unit dependency graph contains a cycle",
        ));
    }
    Ok(waves)
}

#[cfg(test)]
mod tests {
    use super::{UnitSpec, level_waves};

    fn spec(id: &str, deps: &[&str]) -> UnitSpec {
        UnitSpec::new(id, deps.iter().copied())
    }

    #[test]
    fn edgeless_units_collapse_to_one_wave() {
        let units = [spec("c", &[]), spec("a", &[]), spec("b", &[])];
        let waves = level_waves(&units).unwrap();
        // One wide parallel wave, deterministically ordered.
        assert_eq!(waves, vec![vec!["a".to_string(), "b".into(), "c".into()]]);
    }

    #[test]
    fn edges_produce_dependency_ordered_waves() {
        let units = [
            spec("leaf", &["mid"]),
            spec("mid", &["root"]),
            spec("root", &[]),
            spec("sibling", &["root"]),
        ];
        let waves = level_waves(&units).unwrap();
        assert_eq!(
            waves,
            vec![
                vec!["root".to_string()],
                vec!["mid".to_string(), "sibling".into()],
                vec!["leaf".to_string()],
            ]
        );
    }

    #[test]
    fn repeated_edge_does_not_strand_a_unit() {
        let units = [spec("a", &[]), spec("b", &["a", "a"])];
        let waves = level_waves(&units).unwrap();
        assert_eq!(waves, vec![vec!["a".to_string()], vec!["b".to_string()]]);
    }

    #[test]
    fn unknown_dependency_is_rejected() {
        let units = [spec("a", &["ghost"])];
        let err = level_waves(&units).unwrap_err();
        assert!(err.to_string().contains("unknown unit 'ghost'"));
    }

    #[test]
    fn duplicate_id_is_rejected() {
        let units = [spec("a", &[]), spec("a", &[])];
        let err = level_waves(&units).unwrap_err();
        assert!(err.to_string().contains("duplicate unit id 'a'"));
    }

    #[test]
    fn cycle_is_rejected() {
        let units = [spec("a", &["b"]), spec("b", &["a"])];
        let err = level_waves(&units).unwrap_err();
        assert!(err.to_string().contains("cycle"));
    }
}
