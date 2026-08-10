//! Rebase one member's discovery output into the umbrella coordinate space.
//!
//! A member is discovered against its own repo root, so its workspaces,
//! modules, and edges come back member-local: workspace ids unscoped, paths
//! relative to the member root, and edges built from bare (member-unscoped)
//! keys. Before the per-member responses can be unioned into one federated
//! graph they are rebased:
//!
//! - **member stamp** — every module and every edge endpoint is scoped to the
//!   member id, so two members exposing the same `ecosystem:name` stay distinct
//!   ([`Graph::build`](toven_model::Graph::build) would otherwise reject the
//!   duplicate identity);
//! - **workspace scoping** — workspace ids are namespaced by member (two
//!   members each discovering a `rust` workspace would otherwise collide), and
//!   every module's `workspace` reference is rewritten to match;
//! - **path prefixing** — module/workspace roots, manifests, and
//!   change-detection globs are prefixed with the member's path under the
//!   umbrella root, so the single umbrella root that toolchain probing, the
//!   content digest, and command execution all join against resolves every
//!   member-relative path correctly.
//!
//! The degenerate single-repo member has no member id and sits at the umbrella
//! root (empty prefix), so it is never rebased and its discovery output flows
//! through byte-for-byte unchanged.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_model::{MemberId, RepoPath, WorkspaceId};

use super::identity::stamp_modules;
use crate::plan::discover::Federation;

/// Rebase `federation` (one member's discovery output) into umbrella
/// coordinates.
///
/// `prefix` is the member's discovery root relative to the umbrella root.
/// Member and workspace identities are always scoped; an empty prefix means
/// paths stay untouched because the member already sits at the umbrella root.
///
/// # Errors
/// Returns an [`ErrorCode::Internal`] error when discovery emits a module
/// workspace reference that does not exist in the member's workspace set, or
/// propagates a [`RepoPath`] construction failure while prefixing a path.
pub(super) fn rebase_member(
    federation: &mut Federation,
    member: &MemberId,
    prefix: &Path,
) -> AppResult<()> {
    stamp_modules(&mut federation.modules, member);
    stamp_edges(federation, member);
    scope_workspaces(federation, member)?;
    if prefix.as_os_str().is_empty() {
        return Ok(());
    }
    prefix_paths(federation, prefix)
}

/// Scope every edge endpoint to `member`.
///
/// Adapters emit intra-member edges from bare
/// [`ModuleRef`](toven_model::ModuleRef)s, and member-local overlays are
/// appended bare as well, so both endpoints belong to this one member.
fn stamp_edges(federation: &mut Federation, member: &MemberId) {
    for edge in &mut federation.edges {
        edge.from.member = Some(member.clone());
        edge.to.member = Some(member.clone());
    }
}

/// Namespace every workspace id by `member` and rewrite module references to
/// it.
fn scope_workspaces(federation: &mut Federation, member: &MemberId) -> AppResult<()> {
    let mut remap: BTreeMap<WorkspaceId, WorkspaceId> = BTreeMap::new();
    for workspace in &mut federation.workspaces {
        let scoped = scoped_workspace_id(member, &workspace.id)?;
        remap.insert(workspace.id.clone(), scoped.clone());
        workspace.id = scoped;
    }
    for module in &mut federation.modules {
        if let Some(current) = &module.workspace {
            let scoped = remap.get(current).ok_or_else(|| {
                AppError::new(
                    ErrorCode::Internal,
                    format!(
                        "module '{}' in member '{member}' references unknown workspace '{current}'",
                        module.key()
                    ),
                )
            })?;
            module.workspace = Some(scoped.clone());
        }
    }
    Ok(())
}

/// Derive a member-scoped workspace id (`member/<id>`).
fn scoped_workspace_id(member: &MemberId, id: &WorkspaceId) -> AppResult<WorkspaceId> {
    WorkspaceId::new(format!("{member}/{id}")).map_err(|error| {
        AppError::new(
            ErrorCode::Internal,
            format!("could not scope workspace '{id}' to member '{member}'"),
        )
        .with_cause(error)
    })
}

/// Prefix every member-relative path and glob with `prefix`.
fn prefix_paths(federation: &mut Federation, prefix: &Path) -> AppResult<()> {
    for workspace in &mut federation.workspaces {
        workspace.root = rebase_repo_path(prefix, &workspace.root)?;
        prefix_globs(prefix, &mut workspace.blast_radius);
    }
    for module in &mut federation.modules {
        module.root = rebase_repo_path(prefix, &module.root)?;
        if let Some(manifest) = &module.manifest {
            module.manifest = Some(rebase_repo_path(prefix, manifest)?);
        }
        prefix_globs(prefix, &mut module.source_patterns);
    }
    Ok(())
}

/// Join `prefix` in front of a member-relative repo path.
fn rebase_repo_path(prefix: &Path, path: &RepoPath) -> AppResult<RepoPath> {
    RepoPath::new(prefix.join(path.as_path()))
}

/// Prefix each change-detection glob with the member's umbrella-relative path.
///
/// Globs match against umbrella-relative changed paths, so the member directory
/// must lead each pattern; the prefix is rendered with forward slashes to match
/// the slash-separated paths the matcher compares against.
fn prefix_globs(prefix: &Path, globs: &mut [String]) {
    let lead = forward_slashes(prefix);
    for glob in globs {
        *glob = format!("{lead}/{glob}");
    }
}

/// Render a relative path with forward slashes regardless of host separator.
fn forward_slashes(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Compute one member's discovery root relative to the umbrella root.
///
/// Returns an empty path when the member sits at the umbrella root (the
/// degenerate single-repo case), signalling that no path prefixing is needed.
///
/// # Errors
/// Returns an [`ErrorCode::Internal`] error when `discover_root` is not nested
/// under `umbrella_root`, which member enumeration guarantees cannot happen.
pub(super) fn member_prefix(umbrella_root: &Path, discover_root: &Path) -> AppResult<PathBuf> {
    discover_root
        .strip_prefix(umbrella_root)
        .map(Path::to_path_buf)
        .map_err(|source| {
            AppError::new(
                ErrorCode::Internal,
                format!(
                    "member root '{}' is not nested under umbrella root '{}'",
                    discover_root.display(),
                    umbrella_root.display()
                ),
            )
            .with_cause(source)
        })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use toven_model::{
        DepKind, EcosystemId, Edge, MemberId, Module, ModuleRef, RepoPath, ToolchainTag, Workspace,
        WorkspaceId,
    };

    use super::{member_prefix, rebase_member};
    use crate::plan::discover::Federation;

    fn module(name: &str, root: &str, workspace: &str) -> Module {
        let mut module = Module::new(
            ModuleRef::new(EcosystemId::new("rust").unwrap(), name).unwrap(),
            RepoPath::new(root).unwrap(),
        );
        module.workspace = Some(WorkspaceId::new(workspace).unwrap());
        module.manifest = Some(RepoPath::new(format!("{root}/Cargo.toml")).unwrap());
        module
    }

    fn workspace(id: &str, root: &str) -> Workspace {
        let mut workspace = Workspace::new(
            WorkspaceId::new(id).unwrap(),
            RepoPath::new(root).unwrap(),
            ToolchainTag::new("cargo"),
        );
        workspace.blast_radius = vec!["Cargo.lock".to_string()];
        workspace
    }

    fn federation() -> Federation {
        Federation {
            workspaces: vec![workspace("rust", ".")],
            modules: vec![module("core", "crates/core", "rust")],
            edges: vec![Edge::new(
                ModuleRef::new(EcosystemId::new("rust").unwrap(), "core").unwrap(),
                ModuleRef::new(EcosystemId::new("rust").unwrap(), "errors").unwrap(),
                DepKind::Normal,
            )],
            warnings: Vec::new(),
        }
    }

    #[test]
    fn rebase_scopes_identity_and_prefixes_paths() {
        let member = MemberId::new("billing").unwrap();
        let mut federation = federation();

        rebase_member(&mut federation, &member, Path::new("repos/billing")).unwrap();

        let module = &federation.modules[0];
        assert_eq!(module.member.as_ref(), Some(&member));
        assert_eq!(
            module.root.as_path(),
            Path::new("repos/billing/crates/core")
        );
        assert_eq!(
            module.manifest.as_ref().unwrap().as_path(),
            Path::new("repos/billing/crates/core/Cargo.toml")
        );
        assert_eq!(module.workspace.as_ref().unwrap().as_str(), "billing/rust");

        let workspace = &federation.workspaces[0];
        assert_eq!(workspace.id.as_str(), "billing/rust");
        assert_eq!(workspace.root.as_path(), Path::new("repos/billing"));
        assert_eq!(workspace.blast_radius, ["repos/billing/Cargo.lock"]);

        let edge = &federation.edges[0];
        assert_eq!(edge.from.member(), Some(&member));
        assert_eq!(edge.to.member(), Some(&member));
    }

    #[test]
    fn empty_prefix_scopes_identity_without_touching_paths() {
        let member = MemberId::new("solo").unwrap();
        let mut federation = federation();

        rebase_member(&mut federation, &member, Path::new("")).unwrap();

        assert_eq!(
            federation.modules[0].root.as_path(),
            Path::new("crates/core")
        );
        assert_eq!(federation.workspaces[0].id.as_str(), "solo/rust");
    }

    #[test]
    fn member_prefix_is_empty_at_the_umbrella_root() {
        assert!(
            member_prefix(Path::new("/repo"), Path::new("/repo"))
                .unwrap()
                .as_os_str()
                .is_empty()
        );
        assert_eq!(
            member_prefix(Path::new("/repo"), Path::new("/repo/repos/billing")).unwrap(),
            Path::new("repos/billing")
        );
    }

    #[test]
    fn member_prefix_rejects_a_root_outside_the_umbrella() {
        assert!(member_prefix(Path::new("/repo"), Path::new("/elsewhere")).is_err());
    }

    #[test]
    fn rebase_rejects_module_workspace_missing_from_member_discovery() {
        let member = MemberId::new("billing").unwrap();
        let mut federation = federation();
        federation.modules[0].workspace = Some(WorkspaceId::new("missing").unwrap());

        let error = rebase_member(&mut federation, &member, Path::new("repos/billing"))
            .expect_err("unknown workspace reference is rejected");

        assert!(
            error.to_string().contains("unknown workspace 'missing'"),
            "error should identify the missing workspace: {error}"
        );
        assert!(
            error.to_string().contains("billing/rust:core"),
            "error should identify the scoped module key: {error}"
        );
    }
}
