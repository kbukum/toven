//! Edge relaxation and wave leveling: turn the active module set into a
//! topo-levelled wave order and the per-module dependency layer.
//!
//! Per-module `RunStrategy` decides whether an intra-ecosystem ordering edge is
//! kept (`leaf-to-top`) or dropped (`unordered`); overlay edges are always kept.
//! The residual active subgraph is levelled into waves by
//! [`Graph::waves`](toven_model::Graph::waves), and each module's dependency layer
//! is the wave index it first becomes ready in.

use std::collections::BTreeMap;

use rskit_errors::AppResult;
use toven_model::{DepKind, Edge, Graph, Module, ModuleKey, Workspace, WorkspaceId};
use toven_ports::RunStrategy;

use super::task::{EffectiveTask, adapter_for, effective_for};
use crate::plan::configure::MemberAdapters;
use crate::plan::discover::Federation;
use crate::plan::overrides::GroupOverrides;

/// Index the active modules by key.
pub(super) fn active_modules(
    federation: &Federation,
    active: &[ModuleKey],
) -> BTreeMap<ModuleKey, Module> {
    let active: std::collections::BTreeSet<&ModuleKey> = active.iter().collect();
    federation
        .modules
        .iter()
        .filter(|module| active.contains(&module.key()))
        .map(|module| (module.key(), module.clone()))
        .collect()
}

/// Resolve each active module's `RunStrategy`: a group override wins, else the
/// ecosystem override, else the per-kind adapter default.
///
/// The per-kind default is keyed on the module's **resolved effective task
/// kind** (a named extra's true kind, e.g. `Test` for `test-integration`), not
/// the raw user token, so a named extra inherits its kind's ordering policy.
pub(super) fn strategies(
    modules: &BTreeMap<ModuleKey, Module>,
    adapters: &MemberAdapters,
    overrides: &GroupOverrides,
    effective: &BTreeMap<ModuleKey, EffectiveTask>,
) -> AppResult<BTreeMap<ModuleKey, RunStrategy>> {
    let mut strategies = BTreeMap::new();
    for (key, module) in modules {
        let adapter = adapter_for(module, adapters)?;
        let kind = effective_for(key, effective)?.task.kind;
        let strategy = overrides
            .run_strategy(key)
            .or_else(|| adapter.common().run_strategy)
            .unwrap_or_else(|| adapter.run_strategy_default(kind));
        strategies.insert(key.clone(), strategy);
    }
    Ok(strategies)
}

/// Build the validated subgraph spanning only the active modules and their edges.
pub(super) fn active_subgraph(
    modules: &BTreeMap<ModuleKey, Module>,
    federation: &Federation,
) -> AppResult<Graph> {
    let nodes: Vec<Module> = modules.values().cloned().collect();
    let edges: Vec<Edge> = federation
        .edges
        .iter()
        .filter(|edge| modules.contains_key(&edge.from) && modules.contains_key(&edge.to))
        .cloned()
        .collect();
    Graph::build(nodes, edges)
}

/// Whether an edge is kept as an ordering constraint after relaxation.
///
/// Overlay edges are always kept; an intra-ecosystem edge is kept only when its
/// dependent module's strategy is `leaf-to-top`.
pub(super) fn keep_edge(edge: &Edge, strategies: &BTreeMap<ModuleKey, RunStrategy>) -> bool {
    if edge.kind == DepKind::Overlay {
        return true;
    }
    matches!(strategies.get(&edge.from), Some(RunStrategy::LeafToTop))
}

/// Map each active module to the module keys of its kept dependency edges.
///
/// The kept edges are exactly those that ordered the waves (overlay edges plus
/// intra-ecosystem edges retained under `leaf-to-top`); they drive APPLY's
/// fail-closed gating. All endpoints are active, so every id resolves to a unit.
pub(super) fn kept_dependencies(
    modules: &BTreeMap<ModuleKey, Module>,
    federation: &Federation,
    strategies: &BTreeMap<ModuleKey, RunStrategy>,
) -> BTreeMap<ModuleKey, Vec<ModuleKey>> {
    let mut deps: BTreeMap<ModuleKey, Vec<ModuleKey>> = BTreeMap::new();
    for edge in &federation.edges {
        if modules.contains_key(&edge.from)
            && modules.contains_key(&edge.to)
            && keep_edge(edge, strategies)
        {
            deps.entry(edge.from.clone())
                .or_default()
                .push(edge.to.clone());
        }
    }
    deps
}

/// Index discovered workspaces by id.
pub(super) fn workspace_index(federation: &Federation) -> BTreeMap<WorkspaceId, Workspace> {
    federation
        .workspaces
        .iter()
        .map(|workspace| (workspace.id.clone(), workspace.clone()))
        .collect()
}

/// Map each active module to its dependency layer: the wave index it first
/// becomes ready in under the kept-edge relaxation. Reuses the topo-levelled
/// waves ([`Graph::waves`](toven_model::Graph::waves)) — the layer *is* the wave
/// index — rather than re-deriving a topological order.
pub(super) fn layer_index(waves: &[Vec<ModuleKey>]) -> BTreeMap<ModuleKey, usize> {
    let mut layers = BTreeMap::new();
    for (index, wave) in waves.iter().enumerate() {
        for reference in wave {
            layers.insert(reference.clone(), index);
        }
    }
    layers
}
