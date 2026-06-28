//! The cross-repo PLAN spine: compose members, configure each member's adapters,
//! discover per member, and union every member's discovery output into the one
//! federated graph dataset.
//!
//! This is the N-member generalization of the single Configure → Discover front
//! half. The degenerate single-repo project is the same path with one implicit,
//! unscoped member rooted at the project root, so its plan is unchanged.
//!
//! Composition is computed once up front; Configure then bakes every member's
//! `[ecosystems.*]` into per-member adapters (kept apart by member because each
//! member owns its config), and Discover runs each member's adapters against that
//! member's root and rebases the responses into umbrella coordinates before the
//! union. Cross-member overlay edges declared on the umbrella are resolved against
//! the unioned module set and appended last.

use std::collections::BTreeSet;

use rskit_errors::{AppError, AppResult};
use toven_model::{AbsPath, DepKind, EcosystemId, Edge, Module, ModuleKey, ModuleRef};
use toven_ports::Provider;

use crate::config::{CanonicalRegistry, Document, OverlayConfig, OverlayRef};
use crate::federation::rebase::{member_prefix, rebase_member};
use crate::federation::resolve::{self, DriverLocator};
use crate::plan::configure::{self, ConfiguredSet, MemberAdapters};
use crate::plan::discover::{self, Federation};

use super::compose::{ComposedFederation, ComposedMember, compose_members};
use super::members::enumerate_members;

/// Compose the umbrella into its members plus the cross-member overlay layer.
///
/// Enumerates `[[members]]` (or the degenerate single member), then loads each
/// member's authoritative `toven.toml` through the strict engine loader.
///
/// # Errors
/// Propagates member enumeration (absent/escaping member) and composition
/// (missing or invalid member `toven.toml`) failures.
pub(crate) fn compose(
    umbrella_root: &AbsPath,
    document: &Document,
    providers: &[&dyn Provider],
) -> AppResult<ComposedFederation> {
    let loaded: BTreeSet<EcosystemId> = providers
        .iter()
        .map(|provider| provider.ecosystem_id().clone())
        .collect();
    let canonical = CanonicalRegistry::model();
    let members = enumerate_members(document, umbrella_root)?;
    compose_members(document, &members, &loaded, &canonical)
}

/// Configure every member's adapters, returning them keyed by member.
///
/// `warnings` collects every member's absent-driver skips so the caller can
/// surface them while still in the Configure phase.
///
/// # Errors
/// Propagates a member's provider-configure or driver-resolution failure.
pub(crate) fn configure_all(
    composed: &ComposedFederation,
    providers: &[&dyn Provider],
    locator: &dyn DriverLocator,
) -> AppResult<(MemberAdapters, Vec<String>)> {
    let mut adapters = MemberAdapters::default();
    let mut warnings = Vec::new();
    for member in composed.members() {
        let (set, mut member_warnings) = configure_member(member, providers, locator)?;
        warnings.append(&mut member_warnings);
        adapters.insert(member.member().id().cloned(), set);
    }
    Ok((adapters, warnings))
}

/// Configure one member: bake its `[ecosystems.*]` and resolve its out-of-proc
/// drivers behind the same `ConfiguredAdapter` trait.
fn configure_member(
    member: &ComposedMember,
    providers: &[&dyn Provider],
    locator: &dyn DriverLocator,
) -> AppResult<(ConfiguredSet, Vec<String>)> {
    let document = member.document();
    let mut set = configure::configure(document, providers)?;
    let remote = resolve::resolve_adapters(document, providers, locator)?;
    for (id, adapter) in remote.adapters {
        set.insert(id, adapter);
    }
    Ok((set, remote.warnings))
}

/// Discover every member and union the responses into one federated dataset.
///
/// Each member is discovered against its own root, its member-local overlays are
/// appended, and the response is rebased into umbrella coordinates (member-scoped
/// identity + umbrella-relative paths) before being unioned. Cross-member overlay
/// edges declared on the umbrella are resolved against the union and appended.
///
/// # Errors
/// Propagates an adapter discovery failure, a malformed overlay endpoint, an
/// unresolvable cross-member overlay reference, or a duplicate workspace id
/// across the union.
pub(crate) fn discover_all(
    umbrella_root: &AbsPath,
    composed: &ComposedFederation,
    adapters: &MemberAdapters,
) -> AppResult<Federation> {
    let mut union = Federation::default();
    for member in composed.members() {
        let mut member_federation = discover_member(umbrella_root, member, adapters)?;
        union.workspaces.append(&mut member_federation.workspaces);
        union.modules.append(&mut member_federation.modules);
        union.edges.append(&mut member_federation.edges);
        union.warnings.append(&mut member_federation.warnings);
    }

    for overlay in composed.overlays() {
        union
            .edges
            .push(resolve_cross_member_overlay(overlay, &union.modules)?);
    }

    discover::ensure_unique_workspaces(&union.workspaces)?;
    Ok(union)
}

/// Discover one member and rebase its output into umbrella coordinates.
fn discover_member(
    umbrella_root: &AbsPath,
    member: &ComposedMember,
    adapters: &MemberAdapters,
) -> AppResult<Federation> {
    let member_id = member.member().id();
    let set = adapters.set_for(member_id).ok_or_else(|| {
        AppError::new(
            rskit_errors::ErrorCode::Internal,
            format!(
                "member '{}' was not configured before discovery",
                member.member().name()
            ),
        )
    })?;
    let mut federation = discover::union(member.discover_root(), set)?;
    discover::append_overlays(&mut federation, &member.document().overlays)?;
    if let Some(id) = member_id {
        let prefix = member_prefix(umbrella_root.as_path(), member.discover_root().as_path())?;
        rebase_member(&mut federation, id, &prefix)?;
    }
    Ok(federation)
}

/// Resolve one umbrella cross-member overlay into a member-scoped [`Edge`].
///
/// Each endpoint's owning member is inferred by matching its `ecosystem:name`
/// against the unioned module set: a unique match is auto-qualified to that
/// member, and a name exposed by no member (or by several) is a typed error.
fn resolve_cross_member_overlay(overlay: &OverlayConfig, modules: &[Module]) -> AppResult<Edge> {
    let from = resolve_overlay_endpoint("overlays.from", &overlay.from, modules)?;
    let to = resolve_overlay_endpoint("overlays.to", &overlay.to, modules)?;
    Ok(Edge::new(from, to, DepKind::Overlay))
}

/// Resolve one overlay endpoint to the member-scoped key it names.
fn resolve_overlay_endpoint(
    field: &str,
    endpoint: &OverlayRef,
    modules: &[Module],
) -> AppResult<ModuleKey> {
    let reference = ModuleRef::new(endpoint.ecosystem.clone(), &endpoint.module)?;
    let mut matches = modules
        .iter()
        .map(Module::key)
        .filter(|key| key.module() == &reference);
    let Some(first) = matches.next() else {
        return Err(AppError::invalid_input(
            field,
            format!("cross-member overlay references unknown module '{reference}'"),
        ));
    };
    if matches.next().is_some() {
        return Err(AppError::invalid_input(
            field,
            format!(
                "cross-member overlay '{reference}' is exposed by multiple members; the umbrella overlay is ambiguous"
            ),
        ));
    }
    Ok(first)
}
