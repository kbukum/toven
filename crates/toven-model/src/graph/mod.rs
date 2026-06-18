//! Federated dependency graph: construction, validation, and accessors.
//!
//! Traversal algorithms (topo wave-leveling and reverse-dependents closure) live
//! in [`topo`](crate::graph::topo).

mod topo;

use std::collections::BTreeMap;

use rskit_errors::{AppError, AppResult};

use crate::{
    edge::{DepKind, Edge},
    identity::ModuleRef,
    module::Module,
};

/// An immutable, validated module dependency graph.
///
/// Built from a union of modules and edges (intra-ecosystem + cross-ecosystem
/// overlay edges in one list). Construction validates that module identities are
/// unique, every edge endpoint resolves, and the graph is acyclic — so all
/// downstream consumers (affected, scheduling) operate on a sound graph.
#[derive(Debug, Clone)]
pub struct Graph {
    modules: BTreeMap<ModuleRef, Module>,
    edges: Vec<Edge>,
    /// Reverse adjacency: `to` → its dependents `(from, kind)`.
    dependents: BTreeMap<ModuleRef, Vec<(ModuleRef, DepKind)>>,
}

impl Graph {
    /// Build and validate a graph from modules and edges.
    ///
    /// Errors on duplicate module identity, an edge referencing an unknown
    /// module, or a dependency cycle.
    pub fn build(modules: Vec<Module>, edges: Vec<Edge>) -> AppResult<Self> {
        let mut indexed = BTreeMap::new();
        for module in modules {
            let id = module.id.clone();
            if indexed.insert(id.clone(), module).is_some() {
                return Err(AppError::invalid_input(
                    "modules",
                    format!("duplicate module '{id}'"),
                ));
            }
        }

        let mut dependents: BTreeMap<ModuleRef, Vec<(ModuleRef, DepKind)>> = BTreeMap::new();
        for edge in &edges {
            for (role, reference) in [("from", &edge.from), ("to", &edge.to)] {
                if !indexed.contains_key(reference) {
                    return Err(AppError::invalid_input(
                        format!("edges.{role}"),
                        format!("edge references unknown module '{reference}'"),
                    ));
                }
            }
            if edge.from == edge.to {
                return Err(AppError::invalid_input(
                    "edges",
                    format!("module '{}' cannot depend on itself", edge.from),
                ));
            }
            dependents
                .entry(edge.to.clone())
                .or_default()
                .push((edge.from.clone(), edge.kind));
        }

        let graph = Self {
            modules: indexed,
            edges,
            dependents,
        };
        graph.ensure_acyclic()?;
        Ok(graph)
    }

    /// All modules, ordered by identity.
    pub fn modules(&self) -> impl Iterator<Item = &Module> {
        self.modules.values()
    }

    /// All edges in insertion order.
    #[must_use]
    pub fn edges(&self) -> &[Edge] {
        &self.edges
    }

    /// Look up a module by reference.
    #[must_use]
    pub fn module(&self, reference: &ModuleRef) -> Option<&Module> {
        self.modules.get(reference)
    }

    /// Whether the graph contains a module with this identity.
    #[must_use]
    pub fn contains(&self, reference: &ModuleRef) -> bool {
        self.modules.contains_key(reference)
    }

    /// Number of modules.
    #[must_use]
    pub fn len(&self) -> usize {
        self.modules.len()
    }

    /// Whether the graph has no modules.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    pub(crate) fn dependents_of(&self, reference: &ModuleRef) -> &[(ModuleRef, DepKind)] {
        self.dependents.get(reference).map_or(&[], Vec::as_slice)
    }

    fn ensure_acyclic(&self) -> AppResult<()> {
        self.topo_levels(|_| true).map(|_| ())
    }
}
