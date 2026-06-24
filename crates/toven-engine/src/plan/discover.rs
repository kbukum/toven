//! Phase 3 — Discover: full federation union across every loaded ecosystem.
//!
//! Discovery is **always full** — never pruned by changed paths. Each configured
//! adapter returns its `{ workspaces, modules, edges }`; the engine unions them
//! into one federated graph dataset and appends the config-declared overlay edges
//! (`DepKind::Overlay`) so the result is a single graph spanning languages.

use rskit_errors::{AppError, AppResult};
use toven_model::{AbsPath, DepKind, EcosystemId, Edge, Module, ModuleRef, Workspace};
use toven_ports::DiscoverRequest;

use crate::config::Document;

use super::configure::ConfiguredSet;

/// The unioned discovery output across all loaded ecosystems plus overlay edges.
///
/// A plain union (`⋃ workspaces`, `⋃ modules`, `⋃ edges ++ overlay edges`); the
/// `ecosystem:name` module identity guarantees no cross-ecosystem collision.
#[derive(Debug, Clone, Default)]
#[allow(clippy::redundant_pub_crate)]
pub(crate) struct Federation {
    /// Every discovered workspace across ecosystems.
    pub(crate) workspaces: Vec<Workspace>,
    /// Every discovered module across ecosystems.
    pub(crate) modules: Vec<Module>,
    /// Intra-ecosystem edges plus the config overlay edges.
    pub(crate) edges: Vec<Edge>,
    /// Non-fatal warnings surfaced by adapters during discovery.
    pub(crate) warnings: Vec<String>,
}

/// Run discovery across every configured adapter and union the results.
///
/// Each adapter discovers under `project_root`; the responses are concatenated
/// and the `[[overlays]]` config edges are appended as [`DepKind::Overlay`].
///
/// # Errors
/// Propagates any adapter discovery failure or a malformed overlay endpoint.
pub(super) fn discover(
    project_root: &AbsPath,
    adapters: &ConfiguredSet,
    document: &Document,
) -> AppResult<Federation> {
    let request = DiscoverRequest::new(project_root.clone());
    let mut federation = Federation::default();

    for adapter in adapters.values() {
        let mut response = adapter.discover(&request)?;
        federation.workspaces.append(&mut response.workspaces);
        federation.modules.append(&mut response.modules);
        federation.edges.append(&mut response.edges);
        federation.warnings.append(&mut response.warnings);
    }

    for overlay in &document.overlays {
        let from = overlay_ref(&overlay.from.ecosystem, &overlay.from.module)?;
        let to = overlay_ref(&overlay.to.ecosystem, &overlay.to.module)?;
        federation.edges.push(Edge::new(from, to, DepKind::Overlay));
    }

    ensure_unique_workspaces(&federation.workspaces)?;
    Ok(federation)
}

/// Reject duplicate workspace ids across the federation.
///
/// Later phases index workspaces by [`WorkspaceId`](toven_model::WorkspaceId)
/// (schedule, toolchain), so two adapters emitting the same id would silently
/// collapse distinct workspaces and corrupt toolchain probing and cache keys.
fn ensure_unique_workspaces(workspaces: &[Workspace]) -> AppResult<()> {
    let mut seen = std::collections::BTreeSet::new();
    for workspace in workspaces {
        if !seen.insert(&workspace.id) {
            return Err(AppError::invalid_input(
                "workspace.id",
                format!(
                    "duplicate workspace id '{}' across the federation",
                    workspace.id
                ),
            ));
        }
    }
    Ok(())
}

/// Build a [`ModuleRef`] for one structured overlay endpoint.
fn overlay_ref(ecosystem: &EcosystemId, module: &str) -> AppResult<ModuleRef> {
    ModuleRef::new(ecosystem.clone(), module)
}

#[cfg(test)]
mod tests {
    use toven_model::{RepoPath, ToolchainTag, Workspace, WorkspaceId};

    use super::ensure_unique_workspaces;

    fn workspace(id: &str) -> Workspace {
        Workspace::new(
            WorkspaceId::new(id).unwrap(),
            RepoPath::new(".").unwrap(),
            ToolchainTag::new("cargo"),
        )
    }

    #[test]
    fn unique_workspace_ids_pass() {
        let workspaces = vec![workspace("rust"), workspace("go")];
        assert!(ensure_unique_workspaces(&workspaces).is_ok());
    }

    #[test]
    fn duplicate_workspace_ids_are_rejected() {
        let workspaces = vec![workspace("rust"), workspace("rust")];
        assert!(ensure_unique_workspaces(&workspaces).is_err());
    }
}
