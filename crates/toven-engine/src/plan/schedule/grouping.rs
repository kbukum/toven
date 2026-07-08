//! Batch-group identity and the dependency-layer fold that keeps the condensed
//! unit graph acyclic.
//!
//! A `PerModule` task keys one unit per module. A `Batchable`/`WholeWorkspace`
//! task collapses same-ecosystem-and-workspace modules into a shared **base** id
//! ([`group_id`]); the scheduler then partitions each base by dependency layer
//! ([`layered_group_ids`]) so a group only ever holds modules that share a layer
//! and therefore cannot depend on one another. Two guards
//! ([`ensure_distinct_ids`], [`ensure_condensed_acyclic`]) fail closed on any
//! residual collision or cycle.

use std::collections::{BTreeMap, BTreeSet};

use rskit_errors::{AppError, AppResult};
use toven_model::{Module, ModuleKey};
use toven_ports::FanOut;

use super::task::{EffectiveTask, effective_for};
use super::unit::PlannedUnit;

/// The unit id for `module` under `task` (`ecosystem:name#task`, member-prefixed
/// whenever the module belongs to a federation member via [`ModuleKey`]'s
/// `Display`).
fn unit_id(module: &ModuleKey, task: &str) -> String {
    format!("{module}#{task}")
}

/// The id of the unit a module belongs to: its own per-module id for a
/// `PerModule` task, or a shared **base** group id for `Batchable`/`WholeWorkspace`
/// tasks that the scheduler then partitions by dependency layer (see
/// [`layered_group_ids`]).
///
/// A base group id is keyed by `member`, `ecosystem`, **and owning workspace**
/// (`[member/]ecosystem@workspace#task`, or `[member/]ecosystem#task` for a
/// workspace-less module). Keeping the workspace in the key guarantees a collapsed
/// unit never spans workspaces, so the representative's `{workspace.root}` and
/// resolved toolchain identity are valid for every member it carries. When a group
/// task override applies, its scope-qualified identity is folded into the key
/// (`…~identity…`) so members carrying overrides from different declarations — or
/// none — never collapse into one argv. The scheduler folds the module's
/// dependency layer on top of this base to keep the condensed unit graph acyclic.
fn group_id(
    key: &ModuleKey,
    module: &Module,
    task: &str,
    fan_out: FanOut,
    override_group: Option<&str>,
) -> String {
    if fan_out == FanOut::PerModule {
        return unit_id(key, task);
    }
    let ecosystem = &key.module().ecosystem;
    let base = module
        .workspace
        .as_ref()
        .map_or_else(|| ecosystem.to_string(), |ws| format!("{ecosystem}@{ws}"));
    let scope = override_group.map_or_else(|| base.clone(), |group| format!("{base}~{group}"));
    key.member().map_or_else(
        || format!("{scope}#{task}"),
        |member| format!("{member}/{scope}#{task}"),
    )
}

/// Map every active module to its base group id (pre-layer). The scheduler folds
/// each module's dependency layer on top via [`layered_group_ids`].
pub(super) fn group_id_map(
    modules: &BTreeMap<ModuleKey, Module>,
    effective: &BTreeMap<ModuleKey, EffectiveTask>,
) -> AppResult<BTreeMap<ModuleKey, String>> {
    let mut ids = BTreeMap::new();
    for (key, module) in modules {
        let eff = effective_for(key, effective)?;
        ids.insert(
            key.clone(),
            group_id(
                key,
                module,
                eff.task.name.as_str(),
                eff.task.fan_out,
                eff.group.as_deref(),
            ),
        );
    }
    Ok(ids)
}

/// Fold each module's dependency layer into its base group id, so a batch group
/// only ever holds modules that share a layer (and therefore cannot depend on one
/// another). A base scope that spans several layers — the facade back-dependency
/// case, where a workspace's suite crate depends on another workspace that in turn
/// depends on the workspace's base crates — is split one unit per layer, each
/// tagged `~L{layer}`. A base confined to a single layer (the common case, and
/// every `PerModule` id) is returned byte-for-byte unchanged, so ordinary plans
/// render exactly as before.
///
/// `WholeWorkspace` bases are **never** split: a whole-workspace task is one
/// invocation covering the entire workspace, so its members stay collapsed into a
/// single unit (scheduled at their latest wave). A genuine facade cycle between
/// whole-workspace units is therefore irreducible and surfaces from
/// [`ensure_condensed_acyclic`] as a typed error.
pub(super) fn layered_group_ids(
    base_ids: &BTreeMap<ModuleKey, String>,
    layer_of: &BTreeMap<ModuleKey, usize>,
    effective: &BTreeMap<ModuleKey, EffectiveTask>,
) -> AppResult<BTreeMap<ModuleKey, String>> {
    let mut layers_per_base: BTreeMap<&str, BTreeSet<usize>> = BTreeMap::new();
    for (key, base) in base_ids {
        let layer = *layer_of
            .get(key)
            .ok_or_else(|| unlayered_module_error(key))?;
        layers_per_base
            .entry(base.as_str())
            .or_default()
            .insert(layer);
    }

    let mut ids = BTreeMap::new();
    for (key, base) in base_ids {
        let layer = *layer_of
            .get(key)
            .ok_or_else(|| unlayered_module_error(key))?;
        let splittable = effective_for(key, effective)?.task.fan_out == FanOut::Batchable;
        let multi_layer = splittable && layers_per_base[base.as_str()].len() > 1;
        ids.insert(key.clone(), layered_id(base, layer, multi_layer));
    }
    ensure_distinct_ids(base_ids, &ids)?;
    Ok(ids)
}

/// The typed error for a scheduled module absent from the topo-levelled waves.
fn unlayered_module_error(key: &ModuleKey) -> AppError {
    AppError::new(
        rskit_errors::ErrorCode::Internal,
        format!("module '{key}' has no dependency layer in the scheduled waves"),
    )
}

/// A base group id, tagged with its layer only when the base spans several layers.
///
/// The tag is inserted before the `#task` suffix so the task name stays the final
/// segment; a single-layer base is returned unchanged.
fn layered_id(base: &str, layer: usize, multi_layer: bool) -> String {
    if !multi_layer {
        return base.to_string();
    }
    base.rsplit_once('#').map_or_else(
        || format!("{base}~L{layer}"),
        |(prefix, task)| format!("{prefix}~L{layer}#{task}"),
    )
}

/// Fail closed if two *distinct* base groups collapsed onto the same display id,
/// which would silently merge separate units under gating. Modules sharing one
/// base id and layer are the batching we intend; a collision across differing
/// base ids is the pathological case this tripwire rejects.
fn ensure_distinct_ids(
    base_ids: &BTreeMap<ModuleKey, String>,
    ids: &BTreeMap<ModuleKey, String>,
) -> AppResult<()> {
    let mut bases_per_id: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (key, id) in ids {
        if let Some(base) = base_ids.get(key) {
            bases_per_id
                .entry(id.as_str())
                .or_default()
                .insert(base.as_str());
        }
    }
    if let Some((id, bases)) = bases_per_id.iter().find(|(_, bases)| bases.len() > 1) {
        let collided: Vec<&str> = bases.iter().copied().collect();
        return Err(AppError::new(
            rskit_errors::ErrorCode::Internal,
            format!(
                "distinct batch groups {} collapsed onto one unit id '{id}'",
                collided.join(", ")
            ),
        ));
    }
    Ok(())
}

/// The de-duplicated dependency-group ids a unit gates on (excluding itself).
///
/// Order is the first-seen order across `members`; a `BTreeSet` guards membership
/// so de-duplication stays linear rather than quadratic in the edge count.
pub(super) fn group_dependencies(
    id: &str,
    members: &[ModuleKey],
    kept_deps: &BTreeMap<ModuleKey, Vec<ModuleKey>>,
    group_ids: &BTreeMap<ModuleKey, String>,
) -> Vec<String> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut depends_on: Vec<String> = Vec::new();
    for member in members {
        if let Some(deps) = kept_deps.get(member) {
            for dep in deps {
                if let Some(dep_id) = group_ids.get(dep)
                    && dep_id != id
                    && seen.insert(dep_id.as_str())
                {
                    depends_on.push(dep_id.clone());
                }
            }
        }
    }
    depends_on
}

/// Fail closed if the condensed unit graph still contains a cycle after layering,
/// so a future regression surfaces a typed internal error instead of silently
/// serializing or mutually blocking distinct units.
pub(super) fn ensure_condensed_acyclic(units: &[PlannedUnit]) -> AppResult<()> {
    let mut indegree: BTreeMap<&str, usize> =
        units.iter().map(|unit| (unit.id.as_str(), 0)).collect();
    let mut dependents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for unit in units {
        for dependency in &unit.depends_on {
            *indegree.entry(unit.id.as_str()).or_insert(0) += 1;
            dependents
                .entry(dependency.as_str())
                .or_default()
                .push(unit.id.as_str());
        }
    }
    let mut ready: Vec<&str> = indegree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(id, _)| *id)
        .collect();
    let mut resolved = 0usize;
    while let Some(id) = ready.pop() {
        resolved += 1;
        for dependent in dependents.get(id).map_or(&[][..], Vec::as_slice) {
            if let Some(degree) = indegree.get_mut(dependent) {
                *degree -= 1;
                if *degree == 0 {
                    ready.push(dependent);
                }
            }
        }
    }
    if resolved != indegree.len() {
        let cycle: Vec<&str> = indegree
            .iter()
            .filter(|(_, degree)| **degree > 0)
            .map(|(id, _)| *id)
            .collect();
        return Err(AppError::new(
            rskit_errors::ErrorCode::Internal,
            format!(
                "condensed unit graph is cyclic after layering: {}",
                cycle.join(", ")
            ),
        ));
    }
    Ok(())
}
