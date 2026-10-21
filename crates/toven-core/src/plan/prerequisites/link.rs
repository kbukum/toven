//! Link prerequisite units to the units that need them and re-level waves.

use std::collections::{BTreeMap, BTreeSet};

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_model::ExecutionUnit;

use super::expand::{Needs, Node};

/// Add a `depends_on` edge from each unit to the units of its prerequisites,
/// then re-level the merged waves so every unit runs after all of its
/// dependencies.
///
/// A prerequisite node with no unit (its module was skipped for the task) is
/// walked through to its own prerequisites, so a package without a `build`
/// script still has its dependencies built first. Edges inside one unit are
/// dropped. With no prerequisites the waves are returned unchanged.
///
/// # Errors
/// [`ErrorCode::InvalidInput`] when the added edges close a cycle between units
/// (prerequisites over a module dependency cycle).
pub(in crate::plan) fn link(
    units: &mut [ExecutionUnit],
    waves: Vec<Vec<String>>,
    needs: &Needs,
) -> AppResult<Vec<Vec<String>>> {
    if needs.is_empty() {
        return Ok(waves);
    }
    let unit_of: BTreeMap<Node, String> = units
        .iter()
        .flat_map(|unit| {
            unit.members
                .iter()
                .map(|member| ((unit.task.clone(), member.clone()), unit.id.clone()))
        })
        .collect();

    for unit in units.iter_mut() {
        let mut targets = BTreeSet::new();
        for member in &unit.members {
            let node = (unit.task.clone(), member.clone());
            for prerequisite in needs.get(&node).into_iter().flatten() {
                resolve(
                    prerequisite,
                    needs,
                    &unit_of,
                    &mut BTreeSet::new(),
                    &mut targets,
                );
            }
        }
        targets.remove(&unit.id);
        for target in targets {
            if !unit.depends_on.contains(&target) {
                unit.depends_on.push(target);
            }
        }
    }
    level(units, &waves)
}

/// Collect the unit ids that satisfy `node`: its own unit, or, when it has
/// none, the units of its prerequisites.
fn resolve(
    node: &Node,
    needs: &Needs,
    unit_of: &BTreeMap<Node, String>,
    visited: &mut BTreeSet<Node>,
    targets: &mut BTreeSet<String>,
) {
    if let Some(id) = unit_of.get(node) {
        targets.insert(id.clone());
        return;
    }
    if !visited.insert(node.clone()) {
        return;
    }
    for prerequisite in needs.get(node).into_iter().flatten() {
        resolve(prerequisite, needs, unit_of, visited, targets);
    }
}

/// Re-level units so each sits in a later wave than every dependency, keeping
/// its original wave as a floor and the original order within a wave.
fn level(units: &[ExecutionUnit], waves: &[Vec<String>]) -> AppResult<Vec<Vec<String>>> {
    let order: Vec<&str> = waves.iter().flatten().map(String::as_str).collect();
    let floor: BTreeMap<&str, usize> = waves
        .iter()
        .enumerate()
        .flat_map(|(index, wave)| wave.iter().map(move |id| (id.as_str(), index)))
        .collect();
    let edges: BTreeMap<&str, &[String]> = units
        .iter()
        .map(|unit| (unit.id.as_str(), unit.depends_on.as_slice()))
        .collect();

    let mut levels: BTreeMap<&str, usize> = BTreeMap::new();
    for id in &order {
        let mut stack = Vec::new();
        level_of(id, &edges, &floor, &mut levels, &mut stack)?;
    }

    let mut leveled: Vec<Vec<String>> = Vec::new();
    for id in order {
        let index = levels.get(id).copied().unwrap_or_default();
        if leveled.len() <= index {
            leveled.resize_with(index + 1, Vec::new);
        }
        if let Some(wave) = leveled.get_mut(index) {
            wave.push(id.to_string());
        }
    }
    leveled.retain(|wave| !wave.is_empty());
    Ok(leveled)
}

/// The wave of `id`: the larger of its original wave and one past its latest
/// dependency.
fn level_of<'a>(
    id: &'a str,
    edges: &BTreeMap<&'a str, &'a [String]>,
    floor: &BTreeMap<&'a str, usize>,
    levels: &mut BTreeMap<&'a str, usize>,
    stack: &mut Vec<&'a str>,
) -> AppResult<usize> {
    if let Some(level) = levels.get(id) {
        return Ok(*level);
    }
    if let Some(position) = stack.iter().position(|seen| *seen == id) {
        let mut cycle: Vec<&str> = stack.get(position..).unwrap_or_default().to_vec();
        cycle.push(id);
        return Err(AppError::new(
            ErrorCode::InvalidInput,
            format!(
                "task prerequisites form a cycle between units: {}. The modules depend on each other; remove a `depends_on` entry or the module dependency.",
                cycle.join(" -> ")
            ),
        ));
    }
    stack.push(id);
    let mut level = floor.get(id).copied().unwrap_or_default();
    for dependency in edges.get(id).copied().unwrap_or_default() {
        if edges.contains_key(dependency.as_str()) {
            level = level.max(level_of(dependency.as_str(), edges, floor, levels, stack)? + 1);
        }
    }
    stack.pop();
    levels.insert(id, level);
    Ok(level)
}
