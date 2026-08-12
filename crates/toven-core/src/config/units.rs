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
/// unit is a leaf and cannot close a cycle. A depth-first walk with an on-stack
/// set reports the first back-edge it reaches.
fn detect_cycles(units: &BTreeMap<String, CompositeUnitConfig>) -> AppResult<()> {
    let mut visited: BTreeSet<&str> = BTreeSet::new();
    for name in units.keys().map(String::as_str) {
        if visited.contains(name) {
            continue;
        }
        let mut stack: Vec<&str> = Vec::new();
        walk(name, units, &mut visited, &mut stack)?;
    }
    Ok(())
}

/// Depth-first visit of one composite, following composite members only.
fn walk<'a>(
    name: &'a str,
    units: &'a BTreeMap<String, CompositeUnitConfig>,
    visited: &mut BTreeSet<&'a str>,
    stack: &mut Vec<&'a str>,
) -> AppResult<()> {
    if let Some(position) = stack.iter().position(|entry| *entry == name) {
        let mut cycle: Vec<&str> = stack[position..].to_vec();
        cycle.push(name);
        return Err(AppError::invalid_input(
            format!("units.{name}"),
            format!(
                "composite units form a cycle: {}; a chain cannot reference itself directly or \
                 transitively",
                cycle.join(" -> ")
            ),
        ));
    }
    if visited.contains(name) {
        return Ok(());
    }
    stack.push(name);
    if let Some(composite) = units.get(name) {
        for member in composite.chain() {
            if units.contains_key(member) {
                walk(member, units, visited, stack)?;
            }
        }
    }
    stack.pop();
    visited.insert(name);
    Ok(())
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
