//! The validated [`Graph`] type: construction, validation, and accessors.
//!
//! Traversal algorithms (topo wave-leveling and reverse-dependents closure)
//! live in [`topo`](crate::graph::topo).

use std::collections::BTreeMap;

use rskit_errors::{AppError, AppResult};

use crate::{
    edge::{DepKind, Edge},
    identity::ModuleKey,
    module::Module,
};

/// An immutable, validated module dependency graph.
///
/// Built from a union of modules and edges (intra-ecosystem + cross-ecosystem
/// overlay edges in one list). Construction validates that module identities
/// are unique, every edge endpoint resolves, and the graph is acyclic — so all
/// downstream consumers (affected, scheduling) operate on a sound graph.
///
/// Nodes are keyed by [`ModuleKey`]: a single-repo graph keys by bare
/// `ecosystem:name`, while a cross-repo umbrella keys the same `ecosystem:name`
/// from two members under distinct member-scoped keys.
#[derive(Debug, Clone)]
pub struct Graph {
    modules: BTreeMap<ModuleKey, Module>,
    edges: Vec<Edge>,
    /// Forward adjacency: `from` → its dependencies `(to, kind)`.
    dependencies: BTreeMap<ModuleKey, Vec<(ModuleKey, DepKind)>>,
    /// Reverse adjacency: `to` → its dependents `(from, kind)`.
    dependents: BTreeMap<ModuleKey, Vec<(ModuleKey, DepKind)>>,
}

impl Graph {
    /// Build and validate a graph from modules and edges.
    ///
    /// Errors on duplicate module identity, an edge referencing an unknown
    /// module, or a dependency cycle.
    pub fn build(modules: Vec<Module>, edges: Vec<Edge>) -> AppResult<Self> {
        let mut indexed = BTreeMap::new();
        for module in modules {
            let key = module.key();
            if indexed.insert(key.clone(), module).is_some() {
                return Err(AppError::invalid_input(
                    "modules",
                    format!("duplicate module '{key}'"),
                ));
            }
        }

        let mut dependencies: BTreeMap<ModuleKey, Vec<(ModuleKey, DepKind)>> = BTreeMap::new();
        let mut dependents: BTreeMap<ModuleKey, Vec<(ModuleKey, DepKind)>> = BTreeMap::new();
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
            dependencies
                .entry(edge.from.clone())
                .or_default()
                .push((edge.to.clone(), edge.kind));
            dependents
                .entry(edge.to.clone())
                .or_default()
                .push((edge.from.clone(), edge.kind));
        }

        let graph = Self {
            modules: indexed,
            edges,
            dependencies,
            dependents,
        };
        graph.ensure_acyclic()?;
        Ok(graph)
    }

    /// All modules, ordered by key.
    pub fn modules(&self) -> impl Iterator<Item = &Module> {
        self.modules.values()
    }

    /// All edges in insertion order.
    #[must_use]
    pub fn edges(&self) -> &[Edge] {
        &self.edges
    }

    /// Look up a module by key.
    #[must_use]
    pub fn module(&self, key: &ModuleKey) -> Option<&Module> {
        self.modules.get(key)
    }

    /// Whether the graph contains a module with this key.
    #[must_use]
    pub fn contains(&self, key: &ModuleKey) -> bool {
        self.modules.contains_key(key)
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

    pub(crate) fn dependencies_of(&self, key: &ModuleKey) -> &[(ModuleKey, DepKind)] {
        self.dependencies.get(key).map_or(&[], Vec::as_slice)
    }

    pub(crate) fn dependents_of(&self, key: &ModuleKey) -> &[(ModuleKey, DepKind)] {
        self.dependents.get(key).map_or(&[], Vec::as_slice)
    }

    fn ensure_acyclic(&self) -> AppResult<()> {
        self.topo_levels(|_| true).map(|_| ())
    }
}
