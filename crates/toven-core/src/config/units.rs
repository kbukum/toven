//! Structural validation of user-declared composite units (`[units.<name>]`).
//!
//! A composite unit chains member units by name. Structural validation here
//! checks, fail-closed, that every unit name is a safe identifier that does not
//! shadow a built-in, that each member resolves to a known unit (a built-in
//! native capability or another declared composite), and that the composite
//! graph is acyclic. Whether a chain's *behavior* is well-formed is the engine's
//! concern; this pass rejects malformed declarations before any execution.

use std::collections::{BTreeMap, BTreeSet};

use rskit_errors::{AppError, AppResult};
use rskit_validation::input::validate_path_safe_identifier;
use toven_ports::CompositeUnitConfig;

/// The built-in native-capability units a composite may chain by name.
///
/// These are Toven's own release-family capabilities, resolvable without any
/// user declaration. A composite member that is neither one of these nor
/// another declared composite is an unknown unit and fails closed.
pub(super) const BUILTIN_UNITS: &[&str] = &["bump", "tag", "publish", "coverage"];

/// The reserved unit-id separator (mirrors the scheduler's marker); a composite
/// name that reaches a unit id must never contain it.
const UNIT_ID_SEPARATOR: char = '~';

/// Validate every `[units.<name>]` composite: safe, non-shadowing names,
/// known members, and an acyclic composite graph.
pub(super) fn validate_units(units: &BTreeMap<String, CompositeUnitConfig>) -> AppResult<()> {
    let declared: BTreeSet<&str> = units.keys().map(String::as_str).collect();
    for (name, composite) in units {
        let field = format!("units.{name}");
        validate_path_safe_identifier(&field, name)?;
        reject_reserved_separator(&field, name)?;
        reject_builtin_shadow(&field, name)?;
        composite.validate(&field)?;
        validate_members_known(&field, composite, &declared)?;
    }
    detect_cycles(units)
}

/// Reject the reserved [`UNIT_ID_SEPARATOR`] in a composite unit name.
fn reject_reserved_separator(field: &str, name: &str) -> AppResult<()> {
    if name.contains(UNIT_ID_SEPARATOR) {
        return Err(AppError::invalid_input(
            field,
            format!("cannot contain the reserved '{UNIT_ID_SEPARATOR}' character"),
        ));
    }
    Ok(())
}

/// Reject a composite name that shadows a built-in unit.
fn reject_builtin_shadow(field: &str, name: &str) -> AppResult<()> {
    if BUILTIN_UNITS.contains(&name) {
        return Err(AppError::invalid_input(
            field,
            format!("cannot shadow the built-in unit '{name}'"),
        ));
    }
    Ok(())
}

/// Reject any member that is neither a built-in unit nor another declared
/// composite.
fn validate_members_known(
    field: &str,
    composite: &CompositeUnitConfig,
    declared: &BTreeSet<&str>,
) -> AppResult<()> {
    for (index, member) in composite.chain().iter().enumerate() {
        let member = member.as_str();
        if !BUILTIN_UNITS.contains(&member) && !declared.contains(member) {
            return Err(AppError::invalid_input(
                format!("{field}.chain[{index}]"),
                format!(
                    "references unknown unit '{member}'; a member must be a built-in unit ({}) or \
                     another declared composite unit",
                    BUILTIN_UNITS.join(", ")
                ),
            ));
        }
    }
    Ok(())
}

/// Detect a cycle in the composite graph (including a self-reference),
/// failing closed with the offending cycle path.
///
/// Only composite → composite edges form the graph; a member that is a built-in
/// unit is a leaf and cannot close a cycle. An **iterative** depth-first walk
/// with an explicit frame stack reports the first back-edge it reaches. It is
/// deliberately not recursive: config is an untrusted trust-boundary input, so a
/// long acyclic chain (`a0 → a1 → …`) must return the typed validation error,
/// never exhaust the call stack.
fn detect_cycles(units: &BTreeMap<String, CompositeUnitConfig>) -> AppResult<()> {
    // `done` are fully-explored composites (they can never be on a future
    // path); `path`/`on_path` are the composites currently on the DFS stack —
    // an edge back into `on_path` is a cycle.
    let mut done: BTreeSet<&str> = BTreeSet::new();
    for root in units.keys().map(String::as_str) {
        if done.contains(root) {
            continue;
        }
        let mut stack: Vec<Frame<'_>> = vec![Frame {
            name: root,
            next: 0,
        }];
        let mut path: Vec<&str> = vec![root];
        let mut on_path: BTreeSet<&str> = BTreeSet::from([root]);
        while let Some(&Frame { name, next }) = stack.last() {
            let members = units.get(name).map_or(&[][..], CompositeUnitConfig::chain);
            match next_edge(members, next, units, &on_path, &done) {
                Edge::Cycle(member) => return Err(cycle_error(&path, member)),
                Edge::Descend {
                    member,
                    advanced_to,
                } => {
                    set_frame_next(&mut stack, advanced_to);
                    stack.push(Frame {
                        name: member,
                        next: 0,
                    });
                    path.push(member);
                    on_path.insert(member);
                }
                Edge::Exhausted => {
                    stack.pop();
                    if let Some(finished) = path.pop() {
                        on_path.remove(finished);
                        done.insert(finished);
                    }
                }
            }
        }
    }
    Ok(())
}

/// One composite on the iterative DFS stack: the composite and the index of the
/// next member edge to examine.
#[derive(Clone, Copy)]
struct Frame<'a> {
    name: &'a str,
    next: usize,
}

/// The outcome of scanning a composite's remaining member edges.
enum Edge<'a> {
    /// A member re-enters the current path: a cycle closing on `member`.
    Cycle(&'a str),
    /// Descend into composite `member`; the current frame's cursor advances to
    /// `advanced_to`.
    Descend { member: &'a str, advanced_to: usize },
    /// No further composite edge to follow: pop this frame.
    Exhausted,
}

/// Scan `members` from `start`, skipping leaf (built-in) and already-explored
/// composite edges, until it finds a back-edge (cycle) or the next composite to
/// descend into.
fn next_edge<'a>(
    members: &'a [String],
    start: usize,
    units: &'a BTreeMap<String, CompositeUnitConfig>,
    on_path: &BTreeSet<&str>,
    done: &BTreeSet<&str>,
) -> Edge<'a> {
    let mut index = start;
    while let Some(member) = members.get(index) {
        let member = member.as_str();
        index += 1;
        if !units.contains_key(member) {
            continue; // a built-in leaf cannot close a cycle
        }
        if on_path.contains(member) {
            return Edge::Cycle(member);
        }
        if done.contains(member) {
            continue; // already fully explored, acyclic
        }
        return Edge::Descend {
            member,
            advanced_to: index,
        };
    }
    Edge::Exhausted
}

/// Advance the top frame's edge cursor to `next`.
const fn set_frame_next(stack: &mut [Frame<'_>], next: usize) {
    if let Some(frame) = stack.last_mut() {
        frame.next = next;
    }
}

/// Build the typed cyclic-composite error, rendering the cycle path from the
/// point `member` first entered `path` back to `member`.
fn cycle_error(path: &[&str], member: &str) -> AppError {
    let start = path.iter().position(|entry| *entry == member).unwrap_or(0);
    let mut cycle: Vec<&str> = path[start..].to_vec();
    cycle.push(member);
    AppError::invalid_input(
        format!("units.{member}"),
        format!(
            "composite units form a cycle: {}; a chain cannot reference itself directly or \
             transitively",
            cycle.join(" -> ")
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::{CompositeUnitConfig, validate_units};
    use std::collections::BTreeMap;

    fn units(
        entries: impl IntoIterator<Item = (&'static str, &'static [&'static str])>,
    ) -> BTreeMap<String, CompositeUnitConfig> {
        entries
            .into_iter()
            .map(|(name, chain)| (name.to_string(), CompositeUnitConfig::new(chain.to_vec())))
            .collect()
    }

    #[test]
    fn accepts_a_chain_of_built_in_units() {
        validate_units(&units([("release", &["bump", "tag", "publish"][..])])).expect("valid");
    }

    #[test]
    fn accepts_a_composite_referencing_another_composite() {
        let map = units([
            ("release", &["bump", "tag", "publish"][..]),
            ("ship", &["release", "publish"][..]),
        ]);
        validate_units(&map).expect("valid");
    }

    #[test]
    fn rejects_an_unknown_member_unit() {
        let error = validate_units(&units([("ship", &["bump", "smoke-test"][..])])).unwrap_err();
        assert!(error.to_string().contains("unknown unit 'smoke-test'"));
    }

    #[test]
    fn rejects_a_name_that_shadows_a_built_in() {
        let error = validate_units(&units([("bump", &["tag"][..])])).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("shadow the built-in unit 'bump'")
        );
    }

    #[test]
    fn rejects_a_self_referencing_cycle() {
        let error = validate_units(&units([("loop", &["loop"][..])])).unwrap_err();
        assert!(error.to_string().contains("cycle"));
        assert!(error.to_string().contains("loop -> loop"));
    }

    #[test]
    fn rejects_a_mutual_cycle() {
        let map = units([("a", &["b"][..]), ("b", &["a"][..])]);
        let error = validate_units(&map).unwrap_err();
        assert!(error.to_string().contains("cycle"));
    }

    #[test]
    fn accepts_a_deep_acyclic_chain_without_overflowing() {
        // A long linear composite chain (`a0 -> a1 -> ... -> aN -> bump`) is
        // acyclic and must validate via the iterative walk without exhausting
        // the call stack.
        let depth = 100_000;
        let mut map = BTreeMap::new();
        for index in 0..depth {
            let next = if index + 1 == depth {
                "bump".to_string()
            } else {
                format!("a{}", index + 1)
            };
            map.insert(format!("a{index}"), CompositeUnitConfig::new(vec![next]));
        }
        validate_units(&map).expect("deep acyclic chain is valid");
    }

    #[test]
    fn rejects_an_empty_chain() {
        let error = validate_units(&units([("release", &[][..])])).unwrap_err();
        assert!(error.to_string().contains("at least one member"));
    }

    #[test]
    fn rejects_a_reserved_separator_in_the_name() {
        let error = validate_units(&units([("re~lease", &["bump"][..])])).unwrap_err();
        assert!(error.to_string().contains("reserved"));
    }
}
