//! Pure traversal algorithms over a [`Graph`]: wave-leveling and reverse closure.

use std::collections::{BTreeMap, BTreeSet};

use rskit_errors::{AppError, AppResult};

use super::Graph;
use crate::{
    edge::{DepKind, Edge},
    identity::ModuleRef,
};

impl Graph {
    /// Reverse-dependents closure over the seed set.
    ///
    /// Starting from `seeds`, walk reverse edges (`to` → `from`) whose [`DepKind`]
    /// satisfies `include`, returning the seeds plus every transitively affected
    /// dependent. `include` lets a caller distinguish, e.g., a `Dev`-only change
    /// (affects tests) from a `Normal` one (affects downstream builds).
    ///
    /// Errors if a seed is not a known module.
    pub fn closure(
        &self,
        seeds: &BTreeSet<ModuleRef>,
        include: impl Fn(DepKind) -> bool,
    ) -> AppResult<BTreeSet<ModuleRef>> {
        for seed in seeds {
            if !self.contains(seed) {
                return Err(AppError::invalid_input(
                    "affected",
                    format!("seed references unknown module '{seed}'"),
                ));
            }
        }

        let mut affected = seeds.clone();
        let mut pending: Vec<ModuleRef> = seeds.iter().cloned().collect();
        while let Some(current) = pending.pop() {
            for (dependent, kind) in self.dependents_of(&current) {
                if !include(*kind) {
                    continue;
                }
                if affected.insert(dependent.clone()) {
                    pending.push(dependent.clone());
                }
            }
        }
        Ok(affected)
    }

    /// Topologically level the graph into dependency-ordered ready waves.
    ///
    /// Each wave contains modules whose kept dependencies all appear in earlier
    /// waves (leaf-first). The `keep` relaxation hook decides, per edge, whether
    /// it counts as an ordering constraint — `leaf-to-top` keeps intra-ecosystem
    /// edges, `unordered` drops them, and overlay edges are always kept by the
    /// engine. Within a wave, modules are ordered by identity for determinism.
    ///
    /// Errors if a kept-edge cycle remains (defensive — `build` already rejects
    /// cycles over the full edge set, and dropping edges cannot create one).
    pub fn waves(&self, keep: impl Fn(&Edge) -> bool) -> AppResult<Vec<Vec<ModuleRef>>> {
        self.topo_levels(keep)
    }

    pub(super) fn topo_levels(
        &self,
        keep: impl Fn(&Edge) -> bool,
    ) -> AppResult<Vec<Vec<ModuleRef>>> {
        let mut remaining: BTreeMap<ModuleRef, usize> = self
            .modules()
            .map(|module| (module.id.clone(), 0))
            .collect();
        let mut kept_dependents: BTreeMap<ModuleRef, Vec<ModuleRef>> = BTreeMap::new();
        let mut seen_pairs: BTreeSet<(ModuleRef, ModuleRef)> = BTreeSet::new();

        for edge in self.edges() {
            if !keep(edge) {
                continue;
            }
            // Collapse parallel kept edges so a node is counted once per dependency.
            // (`build` already rejects self-edges; a stray one would surface as a cycle.)
            if !seen_pairs.insert((edge.from.clone(), edge.to.clone())) {
                continue;
            }
            *remaining.entry(edge.from.clone()).or_insert(0) += 1;
            kept_dependents
                .entry(edge.to.clone())
                .or_default()
                .push(edge.from.clone());
        }

        let mut ready: BTreeSet<ModuleRef> = remaining
            .iter()
            .filter(|(_, count)| **count == 0)
            .map(|(reference, _)| reference.clone())
            .collect();
        let mut waves = Vec::new();

        while !ready.is_empty() {
            let current = std::mem::take(&mut ready);
            for reference in &current {
                remaining.remove(reference);
            }
            for reference in &current {
                for dependent in kept_dependents
                    .get(reference)
                    .map_or(&[][..], Vec::as_slice)
                {
                    if let Some(count) = remaining.get_mut(dependent) {
                        *count -= 1;
                        if *count == 0 {
                            ready.insert(dependent.clone());
                        }
                    }
                }
            }
            waves.push(current.into_iter().collect());
        }

        if !remaining.is_empty() {
            let cycle = remaining
                .keys()
                .map(ModuleRef::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(AppError::invalid_input(
                "modules",
                format!("dependency cycle detected among: {cycle}"),
            ));
        }
        Ok(waves)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use crate::{
        edge::{DepKind, Edge},
        graph::Graph,
        identity::{EcosystemId, ModuleRef, RepoPath},
        module::Module,
    };

    fn module_ref(ecosystem: &str, name: &str) -> ModuleRef {
        ModuleRef::new(EcosystemId::new(ecosystem).unwrap(), name).unwrap()
    }

    fn module(ecosystem: &str, name: &str) -> Module {
        Module::new(module_ref(ecosystem, name), RepoPath::new(name).unwrap())
    }

    fn edge(from: ModuleRef, to: ModuleRef, kind: DepKind) -> Edge {
        Edge::new(from, to, kind)
    }

    #[test]
    fn build_rejects_cycle() {
        let a = module_ref("rust", "a");
        let b = module_ref("rust", "b");
        let result = Graph::build(
            vec![module("rust", "a"), module("rust", "b")],
            vec![
                edge(a.clone(), b.clone(), DepKind::Normal),
                edge(b, a, DepKind::Normal),
            ],
        );
        assert!(result.is_err());
    }

    #[test]
    fn build_rejects_self_dependency() {
        let a = module_ref("rust", "a");
        let result = Graph::build(
            vec![module("rust", "a")],
            vec![edge(a.clone(), a, DepKind::Normal)],
        );
        assert!(result.is_err());
    }

    #[test]
    fn build_rejects_unresolved_edge() {
        let result = Graph::build(
            vec![module("rust", "a")],
            vec![edge(
                module_ref("rust", "a"),
                module_ref("rust", "missing"),
                DepKind::Normal,
            )],
        );
        assert!(result.is_err());
    }

    #[test]
    fn build_rejects_duplicate_module() {
        let result = Graph::build(vec![module("rust", "a"), module("rust", "a")], vec![]);
        assert!(result.is_err());
    }

    #[test]
    fn waves_order_dependencies_first() {
        // a -> b -> c  (a depends on b depends on c)
        let (a, b, c) = (
            module_ref("rust", "a"),
            module_ref("rust", "b"),
            module_ref("rust", "c"),
        );
        let graph = Graph::build(
            vec![
                module("rust", "a"),
                module("rust", "b"),
                module("rust", "c"),
            ],
            vec![
                edge(a.clone(), b.clone(), DepKind::Normal),
                edge(b.clone(), c.clone(), DepKind::Normal),
            ],
        )
        .unwrap();

        let waves = graph.waves(|_| true).unwrap();
        assert_eq!(waves, vec![vec![c], vec![b], vec![a]]);
    }

    #[test]
    fn relaxation_collapses_to_single_wave() {
        let (a, b, c) = (
            module_ref("rust", "a"),
            module_ref("rust", "b"),
            module_ref("rust", "c"),
        );
        let graph = Graph::build(
            vec![
                module("rust", "a"),
                module("rust", "b"),
                module("rust", "c"),
            ],
            vec![
                edge(a.clone(), b.clone(), DepKind::Normal),
                edge(b.clone(), c.clone(), DepKind::Normal),
            ],
        )
        .unwrap();

        // Drop every edge -> one wave containing all modules.
        let waves = graph.waves(|_| false).unwrap();
        assert_eq!(waves, vec![vec![a, b, c]]);
    }

    #[test]
    fn closure_respects_dep_kind() {
        // app --Dev--> lib ; downstream --Normal--> lib
        let (app, lib, downstream) = (
            module_ref("rust", "app"),
            module_ref("rust", "lib"),
            module_ref("rust", "downstream"),
        );
        let graph = Graph::build(
            vec![
                module("rust", "app"),
                module("rust", "lib"),
                module("rust", "downstream"),
            ],
            vec![
                edge(app.clone(), lib.clone(), DepKind::Dev),
                edge(downstream.clone(), lib.clone(), DepKind::Normal),
            ],
        )
        .unwrap();

        let seeds = BTreeSet::from([lib.clone()]);

        // Normal-only closure reaches the downstream build, not the dev dependent.
        let normal = graph
            .closure(&seeds, |kind| matches!(kind, DepKind::Normal))
            .unwrap();
        assert_eq!(normal, BTreeSet::from([lib.clone(), downstream.clone()]));

        // Including Dev edges additionally reaches the dev dependent.
        let all = graph.closure(&seeds, |_| true).unwrap();
        assert_eq!(all, BTreeSet::from([lib, downstream, app]));
    }

    #[test]
    fn closure_spans_ecosystems_via_overlay() {
        // go:api --Overlay--> rust:shared (a Go module depends on a Rust module)
        let (api, shared) = (module_ref("go", "api"), module_ref("rust", "shared"));
        let graph = Graph::build(
            vec![module("go", "api"), module("rust", "shared")],
            vec![edge(api.clone(), shared.clone(), DepKind::Overlay)],
        )
        .unwrap();

        let affected = graph
            .closure(&BTreeSet::from([shared.clone()]), |_| true)
            .unwrap();
        assert_eq!(affected, BTreeSet::from([shared, api]));
    }

    #[test]
    fn closure_rejects_unknown_seed() {
        let graph = Graph::build(vec![module("rust", "a")], vec![]).unwrap();
        let result = graph.closure(&BTreeSet::from([module_ref("rust", "ghost")]), |_| true);
        assert!(result.is_err());
    }
}
