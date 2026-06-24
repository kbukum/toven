//! Release bump planning.

use std::collections::{BTreeMap, BTreeSet};

use rskit_errors::{AppError, AppResult};
use rskit_version::semver::Version;
use toven_model::{DepKind, Edge, Graph, Module, ModuleRef};
use toven_ports::{ReleaseMutation, ReleaseTarget};

use super::{ChangelogEntry, ReleaseBaseline, ReleaseEntry, ReleaseStrategyName, strategy};

/// Inputs required to build release entries.
pub(super) struct BumpInputs<'a> {
    pub(super) graph: &'a Graph,
    pub(super) modules: &'a [Module],
    pub(super) edges: &'a [Edge],
    pub(super) changed: &'a BTreeSet<ModuleRef>,
    pub(super) baselines: &'a BTreeMap<ModuleRef, ReleaseBaseline>,
    pub(super) changelogs: &'a BTreeMap<ModuleRef, ChangelogEntry>,
    pub(super) targets: &'a BTreeMap<toven_model::EcosystemId, Box<dyn ReleaseTarget>>,
    pub(super) release_strategy: ReleaseStrategyName,
}

/// Build release entries from changed modules and release targets.
pub(super) fn plan_entries(input: &BumpInputs<'_>) -> AppResult<Vec<ReleaseEntry>> {
    let active = input.graph.closure(input.changed, release_closure_edge)?;
    let mut planned_versions = BTreeMap::new();
    let module_by_ref = input
        .modules
        .iter()
        .map(|module| (module.id.clone(), module))
        .collect::<BTreeMap<_, _>>();

    for reference in &active {
        let module = module_by_ref.get(reference).ok_or_else(|| {
            AppError::invalid_input("release.modules", format!("unknown module '{reference}'"))
        })?;
        let target = target_for(input.targets, module)?;
        let current = target.declared_version(module)?;
        let planned = strategy::next_version(input.release_strategy, &current)?;
        planned_versions.insert(reference.clone(), (current, planned));
    }

    let ranks = publish_ranks(input.graph, &active)?;
    let mut entries = Vec::new();
    for reference in &active {
        let module = module_by_ref.get(reference).ok_or_else(|| {
            AppError::invalid_input("release.modules", format!("unknown module '{reference}'"))
        })?;
        let target = target_for(input.targets, module)?;
        let (current_version, planned_version) = planned_versions
            .get(reference)
            .cloned()
            .ok_or_else(|| AppError::invalid_input("release.modules", "missing planned version"))?;
        let dep_floor_updates = dep_floor_updates(reference, input.edges, &planned_versions);
        let mutation = ReleaseMutation {
            new_version: Some(planned_version.clone()),
            dep_floor_updates,
        };
        let published = target.published_versions(module)?;
        let publish_needed = !published.contains(&planned_version);
        entries.push(ReleaseEntry {
            module: reference.clone(),
            current_version,
            planned_version: Some(planned_version),
            mutation,
            publish_needed,
            topo_rank: *ranks.get(reference).unwrap_or(&usize::MAX),
            baseline: input.baselines.get(reference).cloned(),
            changelog: input.changelogs.get(reference).cloned().unwrap_or_else(|| {
                ChangelogEntry::new(reference.clone(), "dependency cascade", Vec::new())
            }),
        });
    }
    entries.sort_by(|left, right| {
        left.topo_rank
            .cmp(&right.topo_rank)
            .then_with(|| left.module.cmp(&right.module))
    });
    Ok(entries)
}

const fn release_closure_edge(kind: DepKind) -> bool {
    !matches!(kind, DepKind::Overlay)
}

fn target_for<'a>(
    targets: &'a BTreeMap<toven_model::EcosystemId, Box<dyn ReleaseTarget>>,
    module: &Module,
) -> AppResult<&'a dyn ReleaseTarget> {
    targets
        .get(&module.id.ecosystem)
        .map(Box::as_ref)
        .ok_or_else(|| {
            AppError::invalid_input(
                "release.target",
                format!("module '{}' has no release target", module.id),
            )
        })
}

fn dep_floor_updates(
    module: &ModuleRef,
    edges: &[Edge],
    planned_versions: &BTreeMap<ModuleRef, (Version, Version)>,
) -> BTreeMap<ModuleRef, Version> {
    edges
        .iter()
        .filter(|edge| {
            &edge.from == module
                && edge.from.ecosystem == edge.to.ecosystem
                && !matches!(edge.kind, DepKind::Overlay)
        })
        .filter_map(|edge| {
            planned_versions
                .get(&edge.to)
                .map(|(_, version)| (edge.to.clone(), version.clone()))
        })
        .collect()
}

fn publish_ranks(
    graph: &Graph,
    active: &BTreeSet<ModuleRef>,
) -> AppResult<BTreeMap<ModuleRef, usize>> {
    let waves = graph.waves(|edge| active.contains(&edge.from) && active.contains(&edge.to))?;
    let mut ranks = BTreeMap::new();
    let mut rank = 0;
    for wave in waves {
        for module in wave {
            if active.contains(&module) {
                ranks.insert(module, rank);
                rank += 1;
            }
        }
    }
    Ok(ranks)
}
