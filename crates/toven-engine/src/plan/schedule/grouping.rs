//! Batch-group identity and the dependency-layer fold that keeps the condensed
//! unit graph acyclic.
//!
//! A `PerModule` task keys one unit per module. A `Batchable`/`WholeWorkspace`
//! task collapses same-ecosystem-and-workspace modules into a shared **base** id
//! ([`group_id`]). The scheduler then splits a base by dependency layer
//! ([`layered_group_ids`]) **only** when that base participates in a cross-group
//! cycle of the condensed base-group graph ([`cyclic_bases`]) — the facade
//! back-dependency shape. A clean single-workspace batch (even one with an
//! internal dependency chain) has no cross-group cycle, so it stays one collapsed
//! unit. [`level_units_into_waves`] then levels the condensed unit graph into
//! dependency-respecting waves, failing closed ([`ensure_distinct_ids`], and the
//! leveler's own cycle check) on any residual collision or cycle.

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
/// tasks that the scheduler splits by dependency layer only when the base is in a
/// cross-group cycle (see [`layered_group_ids`]).
///
/// A base group id is keyed by `member`, `ecosystem`, **and owning workspace**
/// (`[member/]ecosystem@workspace#task`, or `[member/]ecosystem#task` for a
/// workspace-less module). Keeping the workspace in the key guarantees a collapsed
/// unit never spans workspaces, so the representative's `{workspace.root}` and
/// resolved toolchain identity are valid for every member it carries. When a group
/// task override applies, its scope-qualified identity is folded into the key
/// (`…~identity…`) so members carrying overrides from different declarations — or
/// none — never collapse into one argv. When the base participates in a cross-group
/// cycle, the scheduler folds each module's dependency layer on top of this base to
/// break it.
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

/// Fold each module's dependency layer into its base group id **only** for bases
/// that participate in a cross-group cycle of the condensed base-group graph (the
/// facade back-dependency case, where a workspace's suite crate depends on another
/// workspace that in turn depends on the workspace's base crates). Such a base is
/// split one unit per layer, each tagged `~L{layer}`, so the layer-homogeneous
/// pieces order strictly low-to-high and break the cycle.
///
/// A base that is **not** in a cross-group cycle — including a clean single
/// workspace whose modules form an internal dependency chain — is returned
/// byte-for-byte unchanged and stays one collapsed unit, preserving the
/// `cargo check -p a -p b …` batching. Every `PerModule` id is likewise
/// unchanged.
///
/// `WholeWorkspace` bases are **never** split: a whole-workspace task is one
/// invocation covering the entire workspace, so its members stay collapsed into a
/// single unit (scheduled at their latest wave). A genuine facade cycle between
/// whole-workspace units is therefore irreducible and surfaces from
/// [`level_units_into_waves`] as a typed error.
pub(super) fn layered_group_ids(
    base_ids: &BTreeMap<ModuleKey, String>,
    layer_of: &BTreeMap<ModuleKey, usize>,
    effective: &BTreeMap<ModuleKey, EffectiveTask>,
    kept_deps: &BTreeMap<ModuleKey, Vec<ModuleKey>>,
) -> AppResult<BTreeMap<ModuleKey, String>> {
    let any_splittable = effective
        .values()
        .any(|eff| eff.task.fan_out == FanOut::Batchable);
    // Cross-group cycles can only be broken by splitting a `Batchable` base, so
    // skip the reachability walk entirely when nothing is splittable; the layer
    // presence check below still runs to fail closed on an unlayered module.
    let cyclic = if any_splittable {
        cyclic_bases(base_ids, kept_deps)
    } else {
        BTreeSet::new()
    };

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
        let multi_layer =
            splittable && cyclic.contains(base) && layers_per_base[base.as_str()].len() > 1;
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

/// The base group ids that participate in a cross-group cycle of the condensed
/// base-group graph. Kept edges are condensed onto distinct base ids (intra-group
/// self-loops dropped); a base is cyclic iff it reaches itself in that self-loop-
/// free graph — any such cycle spans at least two distinct bases. Only these bases
/// are split by layer; a clean single-workspace batch (even one with an internal
/// dependency chain) yields only self-loops and so stays one collapsed unit.
fn cyclic_bases(
    base_ids: &BTreeMap<ModuleKey, String>,
    kept_deps: &BTreeMap<ModuleKey, Vec<ModuleKey>>,
) -> BTreeSet<String> {
    let mut adjacency: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (from, deps) in kept_deps {
        let Some(from_base) = base_ids.get(from) else {
            continue;
        };
        for to in deps {
            if let Some(to_base) = base_ids.get(to)
                && from_base != to_base
            {
                adjacency
                    .entry(from_base.as_str())
                    .or_default()
                    .insert(to_base.as_str());
            }
        }
    }
    adjacency
        .keys()
        .copied()
        .filter(|base| reaches_self(base, &adjacency))
        .map(str::to_string)
        .collect()
}

/// Whether `start` reaches itself along one or more edges of the self-loop-free
/// condensed graph — i.e. it lies on a cross-group cycle.
fn reaches_self(start: &str, adjacency: &BTreeMap<&str, BTreeSet<&str>>) -> bool {
    let mut stack: Vec<&str> = adjacency
        .get(start)
        .into_iter()
        .flatten()
        .copied()
        .collect();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    while let Some(node) = stack.pop() {
        if node == start {
            return true;
        }
        if seen.insert(node)
            && let Some(next) = adjacency.get(node)
        {
            stack.extend(next.iter().copied());
        }
    }
    false
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

/// Level the condensed unit graph into dependency-respecting waves.
///
/// Each unit is placed exactly one wave after its latest `depends_on` dependency
/// (longest-path leveling over the unit edges), so APPLY's strict wave-by-wave
/// execution never submits a unit before a unit it gates on. Deriving waves from
/// the collapsed **unit** graph — rather than from member module wave-indices —
/// is what keeps an un-split multi-layer batch (pulled to a later wave than any
/// single member's module layer) from inverting an external dependent.
///
/// Fails closed with a typed internal error if the graph still contains a cycle
/// (e.g. an irreducible whole-workspace facade cycle), rather than silently
/// serializing or mutually blocking distinct units.
pub(super) fn level_units_into_waves(units: &[PlannedUnit]) -> AppResult<Vec<Vec<String>>> {
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
    let mut level: BTreeMap<&str, usize> = units.iter().map(|unit| (unit.id.as_str(), 0)).collect();
    let mut ready: Vec<&str> = indegree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(id, _)| *id)
        .collect();
    let mut resolved = 0usize;
    while let Some(id) = ready.pop() {
        resolved += 1;
        let current = level.get(id).copied().unwrap_or(0);
        for dependent in dependents.get(id).map_or(&[][..], Vec::as_slice) {
            level
                .entry(dependent)
                .and_modify(|lvl| *lvl = (*lvl).max(current + 1));
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
    let depth = level.values().copied().max().map_or(0, |lvl| lvl + 1);
    let mut waves: Vec<Vec<String>> = vec![Vec::new(); depth];
    for unit in units {
        waves[level.get(unit.id.as_str()).copied().unwrap_or(0)].push(unit.id.clone());
    }
    waves.retain(|wave| !wave.is_empty());
    Ok(waves)
}
