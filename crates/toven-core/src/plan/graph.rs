//! Graph: build and validate the federated graph, then run semantic config
//! validation against discovered modules.
//!
//! [`toven_model::Graph::build`] already validates unique identity, resolvable
//! edges, and acyclicity (intra-ecosystem + overlay edges in one set). On top
//! of that this phase resolves every `[groups.*]` membership against the
//! now-known module set and enforces group guardrails against the real edges.

use std::collections::{BTreeMap, BTreeSet};

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

/// Every declared `[groups.*]`, by name, with its resolved members.
///
/// Same-named groups from different scopes (a member-local group and an
/// umbrella group both called `core`) merge under the one name, so a
/// `--group core` selection reaches every module either declaration names.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct GroupIndex(BTreeMap<String, GroupEntry>);

/// One group as declared and resolved: what it is, which groups it may depend
/// on, and the modules it owns.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct GroupEntry {
    /// The declared description (the first scope that sets one wins).
    pub description: Option<String>,
    /// Groups the members may depend on (`guardrails.allow_groups`); `None`
    /// leaves group dependencies unrestricted. Same-named groups show the
    /// union of their lists, or `None` when any of them is unrestricted;
    /// each scope still enforces its own list.
    pub allow_groups: Option<BTreeSet<String>>,
    /// The resolved member modules.
    pub members: BTreeSet<ModuleKey>,
}

impl GroupIndex {
    /// Build an index from explicit `(name, members)` pairs.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn from_groups<I, S>(groups: I) -> Self
    where
        I: IntoIterator<Item = (S, BTreeSet<ModuleKey>)>,
        S: Into<String>,
    {
        let mut index = Self::default();
        for (name, members) in groups {
            index.add(
                name.into(),
                GroupEntry {
                    members,
                    ..GroupEntry::default()
                },
            );
        }
        index
    }

    /// The declared group names, sorted.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }

    /// Every `(name, entry)` pair, sorted by name.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &GroupEntry)> {
        self.0.iter().map(|(name, entry)| (name.as_str(), entry))
    }

    fn add(&mut self, name: String, entry: GroupEntry) {
        let merged = match self.0.entry(name) {
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert(entry);
                return;
            }
            std::collections::btree_map::Entry::Occupied(slot) => slot.into_mut(),
        };
        if merged.description.is_none() {
            merged.description = entry.description;
        }
        merged.allow_groups = match (merged.allow_groups.take(), entry.allow_groups) {
            (Some(mut allowed), Some(more)) => {
                allowed.extend(more);
                Some(allowed)
            }
            _ => None,
        };
        merged.members.extend(entry.members);
    }
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
) -> AppResult<(GroupOverrides, GroupIndex)> {
    let mut resolved = (GroupOverrides::default(), GroupIndex::default());
    for member in composed.members() {
        validate_groups(
            graph,
            &member.document().groups,
            GroupScope::Member(member.member().id()),
            &mut resolved,
        )?;
    }
    validate_groups(
        graph,
        composed.groups(),
        GroupScope::Umbrella,
        &mut resolved,
    )?;
    Ok(resolved)
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
/// group's scope overrides and membership against its resolved members.
fn validate_groups(
    graph: &Graph,
    groups: &BTreeMap<String, GroupConfig>,
    scope: GroupScope<'_>,
    (overrides, index): &mut (GroupOverrides, GroupIndex),
) -> AppResult<()> {
    let mut resolved = BTreeMap::new();
    for (name, group) in groups {
        resolved.insert(name.as_str(), resolve_members(name, group, graph, scope)?);
    }
    for (name, group) in groups {
        let members = &resolved[name.as_str()];
        enforce_guardrails(name, group, members, &resolved, graph, scope)?;
        overrides.record(&override_identity(name, scope), group, members)?;
        index.add(
            name.clone(),
            GroupEntry {
                description: group.description.clone(),
                allow_groups: group
                    .guardrails
                    .allow_groups
                    .as_ref()
                    .map(|groups| groups.iter().cloned().collect()),
                members: members.clone(),
            },
        );
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
    groups: &BTreeMap<&str, BTreeSet<ModuleKey>>,
    graph: &Graph,
    scope: GroupScope<'_>,
) -> AppResult<()> {
    let field = format!("groups.{name}.guardrails");
    let guardrails = &group.guardrails;
    let forbid = resolve_refs(&format!("{field}.forbid"), &guardrails.forbid, graph, scope)?;
    let mut allowed = resolve_refs(&format!("{field}.allow"), &guardrails.allow, graph, scope)?;
    for allowed_group in guardrails.allow_groups.iter().flatten() {
        let group_members = groups.get(allowed_group.as_str()).ok_or_else(|| {
            AppError::invalid_input(
                format!("{field}.allow_groups"),
                format!("references unknown group '{allowed_group}'"),
            )
        })?;
        allowed.extend(group_members.iter().cloned());
    }
    let restricted = !guardrails.allow.is_empty() || guardrails.allow_groups.is_some();

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
        if restricted && !members.contains(&edge.to) && !allowed.contains(&edge.to) {
            return Err(AppError::invalid_input(
                &field,
                format!(
                    "group '{name}' does not allow dependency '{}' -> '{}'{}",
                    edge.from,
                    edge.to,
                    owning_groups(&edge.to, groups)
                ),
            ));
        }
    }
    Ok(())
}

/// ` (in group 'a', 'b')` for a key some groups own, else ` (in no group)`.
fn owning_groups(key: &ModuleKey, groups: &BTreeMap<&str, BTreeSet<ModuleKey>>) -> String {
    let owners: Vec<String> = groups
        .iter()
        .filter(|(_, members)| members.contains(key))
        .map(|(name, _)| format!("'{name}'"))
        .collect();
    if owners.is_empty() {
        " (in no group)".to_string()
    } else {
        format!(" (in group {})", owners.join(", "))
    }
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

    use super::{GroupEntry, GroupIndex, GroupScope, build, validate_groups};
    use crate::plan::discover::Federation;
    use crate::plan::overrides::GroupOverrides;

    fn restricted_to(groups: &[&str]) -> GroupEntry {
        GroupEntry {
            allow_groups: Some(groups.iter().map(ToString::to_string).collect()),
            ..GroupEntry::default()
        }
    }

    #[test]
    fn same_named_groups_union_their_allowed_groups() {
        let mut index = GroupIndex::default();
        index.add("apps".to_string(), restricted_to(&["core"]));
        index.add("apps".to_string(), restricted_to(&["infra"]));
        let (_, entry) = index.iter().next().unwrap();
        let allowed: Vec<&str> = entry
            .allow_groups
            .iter()
            .flatten()
            .map(String::as_str)
            .collect();
        assert_eq!(allowed, ["core", "infra"]);
    }

    #[test]
    fn an_unrestricted_same_named_group_shows_the_merge_unrestricted() {
        for (first, second) in [
            (restricted_to(&["core"]), GroupEntry::default()),
            (GroupEntry::default(), restricted_to(&["core"])),
        ] {
            let mut index = GroupIndex::default();
            index.add("apps".to_string(), first);
            index.add("apps".to_string(), second);
            let (_, entry) = index.iter().next().unwrap();
            assert_eq!(entry.allow_groups, None);
        }
    }

    /// Validate one group set in `scope`, discarding the collected overrides.
    fn validate_only(
        graph: &Graph,
        groups: &BTreeMap<String, GroupConfig>,
        scope: GroupScope<'_>,
    ) -> AppResult<()> {
        let mut resolved = (GroupOverrides::default(), GroupIndex::default());
        validate_groups(graph, groups, scope, &mut resolved)
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
            global_inputs: Vec::new(),
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
    fn allow_groups_admits_dependencies_into_the_named_groups_only() {
        let graph = build(&app_depends_on_errors()).unwrap();
        let mut apps = group(&["rust:app"]);
        apps.guardrails.allow_groups = Some(vec!["core".to_string()]);
        let mut groups = one_group("apps", apps.clone());
        groups.insert("core".to_string(), group(&["rust:errors"]));
        assert!(validate_local(&graph, &groups).is_ok());

        apps.guardrails.allow_groups = Some(vec!["other".to_string()]);
        let mut groups = one_group("apps", apps);
        groups.insert("core".to_string(), group(&["rust:errors"]));
        groups.insert("other".to_string(), group(&[]));
        let message = validate_local(&graph, &groups).unwrap_err().to_string();
        assert!(message.contains("does not allow dependency"), "{message}");
        assert!(message.contains("(in group 'core')"), "{message}");
    }

    #[test]
    fn empty_allow_groups_forbids_dependencies_into_any_other_group() {
        let graph = build(&app_depends_on_errors()).unwrap();
        let mut apps = group(&["rust:app"]);
        apps.guardrails.allow_groups = Some(Vec::new());
        let mut groups = one_group("apps", apps);
        groups.insert("core".to_string(), group(&["rust:errors"]));

        let message = validate_local(&graph, &groups).unwrap_err().to_string();
        assert!(message.contains("does not allow dependency"), "{message}");
    }

    #[test]
    fn allow_groups_naming_an_unknown_group_is_rejected() {
        let graph = build(&app_depends_on_errors()).unwrap();
        let mut apps = group(&["rust:app"]);
        apps.guardrails.allow_groups = Some(vec!["missing".to_string()]);

        let message = validate_local(&graph, &one_group("apps", apps))
            .unwrap_err()
            .to_string();
        assert!(message.contains("unknown group 'missing'"), "{message}");
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
