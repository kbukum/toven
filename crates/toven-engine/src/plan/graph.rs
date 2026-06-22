//! Phase 4 — Graph: build + validate the federated graph, then run the SEMANTIC
//! config validation deferred from Load.
//!
//! [`toven_model::Graph::build`] already validates unique identity, resolvable
//! edges, and acyclicity (intra-ecosystem + overlay edges in one set). On top of
//! that this phase resolves every `[groups.*]` membership against the now-known
//! module set and enforces group guardrails against the real edges.

use std::collections::BTreeSet;

use rskit_errors::{AppError, AppResult};
use toven_model::{Graph, ModuleRef};

use crate::config::{Document, GroupConfig};

use super::discover::Federation;

/// Build the validated federated [`Graph`] from the discovery union.
///
/// # Errors
/// Propagates [`Graph::build`] failures: duplicate identity, an edge referencing
/// an unknown module, a self-edge, or a dependency cycle.
pub(super) fn build(federation: &Federation) -> AppResult<Graph> {
    Graph::build(federation.modules.clone(), federation.edges.clone())
}

/// Run the deferred SEMANTIC config validation against the real graph.
///
/// Resolves group membership and enforces `forbid`/`allow` guardrails over the
/// actual edges.
///
/// # Errors
/// A group ref that does not resolve to a real module, a forbidden edge that is
/// actually present, or an external dependency outside a non-empty `allow` list.
pub(super) fn validate_semantics(graph: &Graph, document: &Document) -> AppResult<()> {
    for (name, group) in &document.groups {
        let members = resolve_members(name, group, graph)?;
        enforce_guardrails(name, group, &members, graph)?;
    }
    Ok(())
}

/// Resolve one group's membership entries to real module refs.
fn resolve_members(
    name: &str,
    group: &GroupConfig,
    graph: &Graph,
) -> AppResult<BTreeSet<ModuleRef>> {
    let field = format!("groups.{name}.modules");
    let mut members = BTreeSet::new();
    for entry in &group.modules {
        let reference = resolve_entry(&field, entry, group.ecosystem.as_ref())?;
        if !graph.contains(&reference) {
            return Err(AppError::invalid_input(
                &field,
                format!("group '{name}' references unknown module '{reference}'"),
            ));
        }
        members.insert(reference);
    }
    Ok(members)
}

/// Resolve a single membership entry (qualified, or bare against the default).
fn resolve_entry(
    field: &str,
    entry: &str,
    default_ecosystem: Option<&toven_model::EcosystemId>,
) -> AppResult<ModuleRef> {
    if entry.contains(':') {
        return ModuleRef::parse(entry).map_err(|error| {
            AppError::invalid_input(field, format!("malformed '{entry}': {error}"))
        });
    }
    let ecosystem = default_ecosystem.ok_or_else(|| {
        AppError::invalid_input(
            field,
            format!("bare module '{entry}' needs a group 'ecosystem' default"),
        )
    })?;
    ModuleRef::new(ecosystem.clone(), entry)
}

/// Enforce a group's `forbid`/`allow` guardrails against the real edges.
fn enforce_guardrails(
    name: &str,
    group: &GroupConfig,
    members: &BTreeSet<ModuleRef>,
    graph: &Graph,
) -> AppResult<()> {
    let field = format!("groups.{name}.guardrails");
    let forbid = parse_refs(&format!("{field}.forbid"), &group.guardrails.forbid)?;
    let allow = parse_refs(&format!("{field}.allow"), &group.guardrails.allow)?;

    for edge in graph.edges() {
        if !members.contains(&edge.from) {
            continue;
        }
        if forbid.contains(&edge.to) {
            return Err(AppError::invalid_input(
                &field,
                format!(
                    "group '{name}' forbids dependency '{}' -> '{}'",
                    edge.from, edge.to
                ),
            ));
        }
        if !allow.is_empty() && !members.contains(&edge.to) && !allow.contains(&edge.to) {
            return Err(AppError::invalid_input(
                &field,
                format!(
                    "group '{name}' allowlist excludes dependency '{}' -> '{}'",
                    edge.from, edge.to
                ),
            ));
        }
    }
    Ok(())
}

/// Parse a list of fully-qualified `ecosystem:module` guardrail refs.
fn parse_refs(field: &str, refs: &[String]) -> AppResult<BTreeSet<ModuleRef>> {
    refs.iter()
        .map(|value| {
            ModuleRef::parse(value).map_err(|error| {
                AppError::invalid_input(field, format!("malformed '{value}': {error}"))
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use toven_model::{DepKind, EcosystemId, Edge, Graph, Module, ModuleRef, RepoPath};

    use crate::config::{Document, GroupConfig, Guardrails, ProjectConfig, TovenConfig};

    use super::{build, validate_semantics};
    use crate::plan::discover::Federation;

    fn mref(ecosystem: &str, name: &str) -> ModuleRef {
        ModuleRef::new(EcosystemId::new(ecosystem).unwrap(), name).unwrap()
    }

    fn module(ecosystem: &str, name: &str) -> Module {
        Module::new(mref(ecosystem, name), RepoPath::new(name).unwrap())
    }

    fn federation(modules: Vec<Module>, edges: Vec<Edge>) -> Federation {
        Federation {
            workspaces: Vec::new(),
            modules,
            edges,
            warnings: Vec::new(),
        }
    }

    fn document(groups: BTreeMap<String, GroupConfig>) -> Document {
        Document {
            project: ProjectConfig {
                name: "t".to_string(),
                root: ".".to_string(),
                base_ref: None,
            },
            toven: TovenConfig::default(),
            groups,
            overlays: Vec::new(),
            ecosystems: BTreeMap::new(),
            members: Vec::new(),
        }
    }

    fn group(modules: &[&str]) -> GroupConfig {
        GroupConfig {
            ecosystem: None,
            modules: modules.iter().map(ToString::to_string).collect(),
            guardrails: Guardrails::default(),
        }
    }

    fn app_depends_on_errors() -> Federation {
        federation(
            vec![module("rust", "app"), module("rust", "errors")],
            vec![Edge::new(
                mref("rust", "app"),
                mref("rust", "errors"),
                DepKind::Normal,
            )],
        )
    }

    #[test]
    fn valid_group_membership_passes_validation() {
        let graph: Graph = build(&app_depends_on_errors()).unwrap();
        let mut groups = BTreeMap::new();
        groups.insert("apps".to_string(), group(&["rust:app"]));
        groups.insert("core".to_string(), group(&["rust:errors"]));

        assert!(validate_semantics(&graph, &document(groups)).is_ok());
    }

    #[test]
    fn allowlist_excluding_a_real_dependency_is_rejected() {
        let graph = build(&app_depends_on_errors()).unwrap();
        // `app` really depends on `errors`, but the allowlist omits it.
        let mut restricted = group(&["rust:app"]);
        restricted.guardrails.allow = vec!["rust:other".to_string()];
        let mut groups = BTreeMap::new();
        groups.insert("apps".to_string(), restricted);

        assert!(validate_semantics(&graph, &document(groups)).is_err());
    }

    #[test]
    fn forbidden_dependency_is_rejected() {
        let graph = build(&app_depends_on_errors()).unwrap();
        let mut forbidding = group(&["rust:app"]);
        forbidding.guardrails.forbid = vec!["rust:errors".to_string()];
        let mut groups = BTreeMap::new();
        groups.insert("apps".to_string(), forbidding);

        assert!(validate_semantics(&graph, &document(groups)).is_err());
    }

    #[test]
    fn unknown_group_member_is_rejected() {
        let graph = build(&app_depends_on_errors()).unwrap();
        let mut groups = BTreeMap::new();
        groups.insert("ghosts".to_string(), group(&["rust:ghost"]));

        assert!(validate_semantics(&graph, &document(groups)).is_err());
    }
}
