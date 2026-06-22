//! Phase 3 — Discover: full federation union across every loaded ecosystem.
//!
//! Discovery is **always full** — never pruned by changed paths. Each configured
//! adapter returns its `{ workspaces, modules, edges }`; the engine unions them
//! into one federated graph dataset and appends the config-declared overlay edges
//! (`DepKind::Overlay`) so the result is a single graph spanning languages.

use rskit_errors::AppResult;
use toven_model::{AbsPath, DepKind, EcosystemId, Edge, Module, ModuleRef, Workspace};
use toven_ports::DiscoverRequest;

use crate::config::Document;

use super::configure::ConfiguredSet;

/// The unioned discovery output across all loaded ecosystems plus overlay edges.
///
/// A plain union (`⋃ workspaces`, `⋃ modules`, `⋃ edges ++ overlay edges`); the
/// `ecosystem:name` module identity guarantees no cross-ecosystem collision.
#[derive(Debug, Clone, Default)]
pub(super) struct Federation {
    /// Every discovered workspace across ecosystems.
    pub(super) workspaces: Vec<Workspace>,
    /// Every discovered module across ecosystems.
    pub(super) modules: Vec<Module>,
    /// Intra-ecosystem edges plus the config overlay edges.
    pub(super) edges: Vec<Edge>,
    /// Non-fatal warnings surfaced by adapters during discovery.
    pub(super) warnings: Vec<String>,
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

    Ok(federation)
}

/// Build a [`ModuleRef`] for one structured overlay endpoint.
fn overlay_ref(ecosystem: &EcosystemId, module: &str) -> AppResult<ModuleRef> {
    ModuleRef::new(ecosystem.clone(), module)
}
