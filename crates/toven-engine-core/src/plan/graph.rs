//! Graph: build and validate the federated graph, then run semantic config
//! validation against discovered modules.
//!
//! [`toven_model::Graph::build`] already validates unique identity, resolvable
//! edges, and acyclicity (intra-ecosystem + overlay edges in one set). On top
//! of that this phase resolves every `[groups.*]` membership against the
//! now-known module set and enforces group guardrails against the real edges.

use std::collections::BTreeSet;

use rskit_errors::{AppError, AppResult};
use toven_model::{Graph, MemberId, ModuleKey, ModuleRef};

use crate::config::GroupConfig;
use crate::federation::compose::ComposedFederation;

use super::discover::Federation;
use super::overrides::GroupOverrides;

/// Build the validated federated [`Graph`] from the discovery union.
///
/// # Errors
/// Propagates [`Graph::build`] failures: duplicate identity, an edge
/// referencing an unknown module, a self-edge, or a dependency cycle.
pub(super) fn build(federation: &Federation) -> AppResult<Graph> {
    Graph::build(federation.modules.clone(), federation.edges.clone())
}

/// Run semantic config validation against the real graph, returning the
/// resolved per-module group overrides gathered along the way.
///
/// Groups are validated in the coordinate space they were declared in: each
/// member's own `[groups.*]` resolve against that member (bare refs bind to the
/// member's own modules), while the umbrella's cross-member `[groups.*]`
/// resolve against the whole union with optional `member/` qualifiers. The
/// degenerate single-repo project is one member with no id, so its groups
/// resolve to bare keys exactly as before and the umbrella layer is empty.
///
/// The same membership resolution feeds group scope overrides
/// ([`GroupOverrides`]): a group's `run_strategy`/`tasks` are recorded against
/// every resolved member so scheduling can layer them on the ecosystem
/// defaults.
///
/// # Errors
/// A group ref that does not resolve to a real module, a forbidden edge that is
/// actually present, an external dependency outside a non-empty `allow` list,
/// or two overlapping groups that override the same module's
/// task/`run_strategy`.
pub(super) fn validate_semantics(
    graph: &Graph,
    composed: &ComposedFederation,
) -> AppResult<GroupOverrides> {
    let mut overrides = GroupOverrides::default();
    for member in composed.members() {
        validate_groups(
            graph,
            &member.document().groups,
            GroupScope::Member(member.member().id()),
            &mut overrides,
        )?;
    }
    validate_groups(
        graph,
        composed.groups(),
        GroupScope::Umbrella,
        &mut overrides,
    )?;
    Ok(overrides)
}

/// The coordinate scope a group's references resolve in.
#[derive(Clone, Copy)]
enum GroupScope<'a> {
    /// A member-local group: every reference binds to this one member. The id
    /// is `None` for the degenerate single-repo project, giving bare keys.
    Member(Option<&'a MemberId>),
    /// An umbrella cross-member group: references may carry an optional
    /// `member/` qualifier and bare references resolve against the union when
    /// unambiguous.
    Umbrella,
}

/// Validate one group set (membership + guardrails) in `scope`, recording each
/// group's scope overrides against its resolved members.
fn validate_groups(
    graph: &Graph,
    groups: &std::collections::BTreeMap<String, GroupConfig>,
    scope: GroupScope<'_>,
    overrides: &mut GroupOverrides,
) -> AppResult<()> {
    for (name, group) in groups {
        let members = resolve_members(name, group, graph, scope)?;
        enforce_guardrails(name, group, &members, graph, scope)?;
        overrides.record(&override_identity(name, scope), group, &members)?;
    }
    Ok(())
}

/// The scope-qualified, id-safe identity of a group declaration.
///
/// It distinguishes two declarations that merely share a plain `name` — a
/// member-local group and an umbrella group both called `test`, or same-named
/// groups in two different members — so overlapping them on one module fails
/// closed, and so members overridden by distinct declarations never collapse
/// into the same batch unit id (the identity is folded into that id verbatim).
///
/// The degenerate single-repo case (`Member(None)`) yields the bare group name,
/// keeping its unit ids stable; federated scopes carry a `member.<id>.` or
/// `umbrella.` prefix. The joiner is `.`, which is id-safe and is not a unit-id
/// separator.
fn override_identity(name: &str, scope: GroupScope<'_>) -> String {
    match scope {
        GroupScope::Member(None) => name.to_string(),
        GroupScope::Member(Some(member)) => format!("member.{member}.{name}"),
        GroupScope::Umbrella => format!("umbrella.{name}"),
    }
}

/// Resolve one group's membership entries to real module keys.
fn resolve_members(
    name: &str,
    group: &GroupConfig,
    graph: &Graph,
    scope: GroupScope<'_>,
) -> AppResult<BTreeSet<ModuleKey>> {
    let field = format!("groups.{name}.modules");
    let mut members = BTreeSet::new();
    for entry in &group.modules {
        let key = resolve_ref(&field, entry, group.ecosystem.as_ref(), graph, scope)?;
        members.insert(key);
    }
    Ok(members)
}

/// Resolve one membership/guardrail entry to a concrete graph key in `scope`.
fn resolve_ref(
    field: &str,
    entry: &str,
    default_ecosystem: Option<&toven_model::EcosystemId>,
    graph: &Graph,
    scope: GroupScope<'_>,
) -> AppResult<ModuleKey> {
    match scope {
        GroupScope::Member(member) => {
            // A member-local entry names a module within its own member, so it never
            // carries a `member/` qualifier; the whole entry is the module reference and
            // the owning member is fixed.
            let reference = parse_module_ref(field, entry.to_string(), default_ecosystem)?;
            let key = ModuleKey::new(member.cloned(), reference);
            if graph.contains(&key) {
                Ok(key)
            } else {
                Err(AppError::invalid_input(
                    field,
                    format!("references unknown module '{key}'"),
                ))
            }
        }
        GroupScope::Umbrella => {
            let (member, reference) = parse_entry(field, entry, default_ecosystem)?;
            resolve_in_graph(field, graph, member.as_ref(), &reference)
        }
    }
}

/// Resolve a single membership entry into an optional member qualifier and a
/// two-level [`ModuleRef`] (qualified, or bare against the group default).
fn parse_entry(
    field: &str,
    entry: &str,
    default_ecosystem: Option<&toven_model::EcosystemId>,
) -> AppResult<(Option<toven_model::MemberId>, ModuleRef)> {
    let (member, rest) = split_member_qualifier(field, entry)?;
    let reference = parse_module_ref(field, rest, default_ecosystem)?;
    Ok((member, reference))
}

/// Split an optional leading `member/` qualifier off a reference string.
fn split_member_qualifier(
    field: &str,
    entry: &str,
) -> AppResult<(Option<toven_model::MemberId>, String)> {
    match entry.split_once('/') {
        Some((member, rest)) => {
            let member = toven_model::MemberId::new(member).map_err(|error| {
                AppError::invalid_input(field, format!("malformed member qualifier in '{entry}'"))
                    .with_cause(error)
            })?;
            Ok((Some(member), rest.to_string()))
        }
        None => Ok((None, entry.to_string())),
    }
}

/// Parse the `ecosystem:module` (or bare `module` against the default) portion.
fn parse_module_ref(
    field: &str,
    entry: String,
    default_ecosystem: Option<&toven_model::EcosystemId>,
) -> AppResult<ModuleRef> {
    if entry.contains(':') {
        return ModuleRef::parse(&entry).map_err(|error| {
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

/// Resolve a `(member?, module)` reference to a concrete graph key.
///
/// A member-qualified ref must match exactly; an unqualified ref resolves when
/// a single member exposes that `ecosystem:name`, and is rejected as ambiguous
/// when two members collide (the caller must add a `member/` qualifier).
fn resolve_in_graph(
    field: &str,
    graph: &Graph,
    member: Option<&toven_model::MemberId>,
    reference: &ModuleRef,
) -> AppResult<ModuleKey> {
    if let Some(member) = member {
        let key = ModuleKey::new(Some(member.clone()), reference.clone());
        if graph.contains(&key) {
            return Ok(key);
        }
        return Err(AppError::invalid_input(
            field,
            format!("references unknown module '{key}'"),
        ));
    }
    let mut matches = graph
        .modules()
        .map(toven_model::Module::key)
        .filter(|key| &key.module == reference);
    let Some(first) = matches.next() else {
        return Err(AppError::invalid_input(
            field,
            format!("references unknown module '{reference}'"),
        ));
    };
    if matches.next().is_some() {
        return Err(AppError::invalid_input(
            field,
            format!(
                "module '{reference}' is exposed by multiple members; qualify it as 'member/{reference}'"
            ),
        ));
    }
    Ok(first)
}

/// Enforce a group's `forbid`/`allow` guardrails against the real edges.
fn enforce_guardrails(
    name: &str,
    group: &GroupConfig,
    members: &BTreeSet<ModuleKey>,
    graph: &Graph,
    scope: GroupScope<'_>,
) -> AppResult<()> {
    let field = format!("groups.{name}.guardrails");
    let forbid = resolve_refs(
        &format!("{field}.forbid"),
        &group.guardrails.forbid,
        graph,
        scope,
    )?;
    let allow = resolve_refs(
        &format!("{field}.allow"),
        &group.guardrails.allow,
        graph,
        scope,
    )?;

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

/// Resolve a list of (optionally member-qualified) guardrail refs to graph
/// keys.
fn resolve_refs(
    field: &str,
    refs: &[String],
    graph: &Graph,
    scope: GroupScope<'_>,
) -> AppResult<BTreeSet<ModuleKey>> {
    refs.iter()
        .map(|value| resolve_ref(field, value, None, graph, scope))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use rskit_errors::AppResult;
    use toven_model::{
        DepKind, EcosystemId, Edge, Graph, MemberId, Module, ModuleKey, ModuleRef, RepoPath,
    };

    use crate::config::GroupConfig;

    use super::{GroupScope, build, validate_groups};
    use crate::plan::discover::Federation;
    use crate::plan::overrides::GroupOverrides;

    /// Validate one group set in `scope`, discarding the collected overrides.
    fn validate_only(
        graph: &Graph,
        groups: &BTreeMap<String, GroupConfig>,
        scope: GroupScope<'_>,
    ) -> AppResult<()> {
        let mut overrides = GroupOverrides::default();
        validate_groups(graph, groups, scope, &mut overrides)
    }

    fn mref(ecosystem: &str, name: &str) -> ModuleRef {
        ModuleRef::new(EcosystemId::new(ecosystem).unwrap(), name).unwrap()
    }

    fn module(ecosystem: &str, name: &str) -> Module {
        Module::new(mref(ecosystem, name), RepoPath::new(name).unwrap())
    }

    fn member_module(member: &str, ecosystem: &str, name: &str) -> Module {
        let mut module = Module::new(
            mref(ecosystem, name),
            RepoPath::new(format!("repos/{member}/{name}")).unwrap(),
        );
        module.member = Some(MemberId::new(member).unwrap());
        module
    }

    fn federation(modules: Vec<Module>, edges: Vec<Edge>) -> Federation {
        Federation {
            workspaces: Vec::new(),
            modules,
            edges,
            warnings: Vec::new(),
        }
    }

    fn group(modules: &[&str]) -> GroupConfig {
        GroupConfig {
            modules: modules.iter().map(ToString::to_string).collect(),
            ..GroupConfig::default()
        }
    }

    fn one_group(name: &str, group: GroupConfig) -> BTreeMap<String, GroupConfig> {
        let mut groups = BTreeMap::new();
        groups.insert(name.to_string(), group);
        groups
    }

    /// Validate a degenerate single-repo group set (member id `None`, bare
    /// keys).
    fn validate_local(graph: &Graph, groups: &BTreeMap<String, GroupConfig>) -> AppResult<()> {
        validate_only(graph, groups, GroupScope::Member(None))
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

        assert!(validate_local(&graph, &groups).is_ok());
    }

    #[test]
    fn allowlist_excluding_a_real_dependency_is_rejected() {
        let graph = build(&app_depends_on_errors()).unwrap();
        // `app` really depends on `errors`, but the allowlist omits it.
        let mut restricted = group(&["rust:app"]);
        restricted.guardrails.allow = vec!["rust:other".to_string()];

        assert!(validate_local(&graph, &one_group("apps", restricted)).is_err());
    }

    #[test]
    fn forbidden_dependency_is_rejected() {
        let graph = build(&app_depends_on_errors()).unwrap();
        let mut forbidding = group(&["rust:app"]);
        forbidding.guardrails.forbid = vec!["rust:errors".to_string()];

        assert!(validate_local(&graph, &one_group("apps", forbidding)).is_err());
    }

    #[test]
    fn unknown_group_member_is_rejected() {
        let graph = build(&app_depends_on_errors()).unwrap();

        assert!(validate_local(&graph, &one_group("ghosts", group(&["rust:ghost"]))).is_err());
    }

    fn two_members_each_with_core() -> Federation {
        let billing = MemberId::new("billing").unwrap();
        federation(
            vec![
                member_module("billing", "rust", "core"),
                member_module("billing", "rust", "api"),
                member_module("catalog", "rust", "core"),
            ],
            vec![Edge::new(
                ModuleKey::bare(mref("rust", "api")).with_member(billing.clone()),
                ModuleKey::bare(mref("rust", "core")).with_member(billing),
                DepKind::Normal,
            )],
        )
    }

    #[test]
    fn member_local_group_binds_bare_refs_to_its_own_member() {
        let graph = build(&two_members_each_with_core()).unwrap();
        let billing = MemberId::new("billing").unwrap();
        // `rust:core` is exposed by both members, but a member-local group resolves it
        // to that member's own module without a qualifier.
        let groups = one_group("core", group(&["rust:core"]));

        assert!(validate_only(&graph, &groups, GroupScope::Member(Some(&billing))).is_ok());
    }

    #[test]
    fn member_local_forbid_only_sees_its_own_members_edge() {
        let graph = build(&two_members_each_with_core()).unwrap();
        let billing = MemberId::new("billing").unwrap();
        let mut forbidding = group(&["rust:api"]);
        forbidding.guardrails.forbid = vec!["rust:core".to_string()];

        assert!(
            validate_only(
                &graph,
                &one_group("apps", forbidding),
                GroupScope::Member(Some(&billing))
            )
            .is_err()
        );
    }

    #[test]
    fn umbrella_group_rejects_an_ambiguous_bare_ref() {
        let graph = build(&two_members_each_with_core()).unwrap();
        // Across the union `rust:core` is exposed by two members, so an umbrella group
        // must qualify it; a bare ref is ambiguous.
        let groups = one_group("shared", group(&["rust:core"]));

        let error =
            validate_only(&graph, &groups, GroupScope::Umbrella).expect_err("ambiguous bare ref");
        assert!(error.to_string().contains("multiple members"));
    }

    #[test]
    fn umbrella_group_resolves_a_member_qualified_ref() {
        let graph = build(&two_members_each_with_core()).unwrap();
        let groups = one_group("shared", group(&["billing/rust:core", "catalog/rust:core"]));

        assert!(validate_only(&graph, &groups, GroupScope::Umbrella).is_ok());
    }
}
