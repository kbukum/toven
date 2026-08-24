//! Batch-group identity and the dependency-layer fold that condenses the unit
//! graph into ordered waves.
//!
//! A `PerModule` task keys one unit per module. A `Batchable`/`WholeWorkspace`
//! task collapses same-ecosystem-and-workspace modules into a shared **base**
//! id ([`group_id`]). The scheduler then splits a base by dependency layer
//! ([`layered_group_ids`]) **only** when that base participates in a
//! cross-group cycle of the condensed base-group graph ([`cyclic_bases`]) — the
//! facade back-dependency shape — and only when the base is `Batchable` (a
//! whole-workspace invocation is atomic and cannot be split). A clean
//! single-workspace batch (even one with an internal dependency chain) has no
//! cross-group cycle, so it stays one collapsed unit. [`level_units_into_waves`]
//! then condenses the strongly-connected components of the unit graph and
//! levels them into dependency-respecting waves: an acyclic graph levels
//! exactly as a longest-path topo-level would, while an irreducible facade
//! cycle co-schedules into a single wave **only** when every unit in it is a
//! whole-workspace invocation — a cycle touching any other unit still fails
//! closed. The mutual edges inside a co-scheduled cycle are stripped so its
//! concurrent peers do not gate on one another in APPLY. [`ensure_distinct_ids`]
//! still fails closed on a residual id collision.

use std::collections::{BTreeMap, BTreeSet};

use rskit_errors::{AppError, AppResult};
use toven_model::{Module, ModuleKey};
use toven_ports::FanOut;

use super::task::{EffectiveTask, effective_for};
use super::unit::PlannedUnit;

/// The unit id for `module` under `task` (`ecosystem:name#task`,
/// member-prefixed whenever the module belongs to a federation member via
/// [`ModuleKey`]'s `Display`).
fn unit_id(module: &ModuleKey, task: &str) -> String {
    format!("{module}#{task}")
}

/// The id of the unit a module belongs to: its own per-module id for a
/// `PerModule` task, or a shared **base** group id for
/// `Batchable`/`WholeWorkspace` tasks that the scheduler splits by dependency
/// layer only when the base is in a cross-group cycle (see
/// [`layered_group_ids`]).
///
/// A base group id is keyed by `member`, `ecosystem`, **and owning workspace**
/// (`[member/]ecosystem@workspace#task`, or `[member/]ecosystem#task` for a
/// workspace-less module). Keeping the workspace in the key guarantees a
/// collapsed unit never spans workspaces, so the representative's
/// `{workspace.root}` and resolved toolchain identity are valid for every
/// member it carries. When a group task override applies, its scope-qualified
/// identity is folded into the key (`…~identity…`) so members carrying
/// overrides from different declarations — or none — never collapse into one
/// argv. When the base participates in a cross-group cycle, the scheduler folds
/// each module's dependency layer on top of this base to break it.
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

/// Map every active module to its base group id (pre-layer). The scheduler
/// folds each module's dependency layer on top via [`layered_group_ids`].
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

/// Fold each module's dependency layer into its base group id **only** for
/// bases that participate in a cross-group cycle of the condensed base-group
/// graph (the facade back-dependency case, where a workspace's suite crate
/// depends on another workspace that in turn depends on the workspace's base
/// crates). Such a base is split one unit per layer, each tagged `~~L{layer}`,
/// so the layer-homogeneous pieces order strictly low-to-high and break the
/// cycle.
///
/// A base that is **not** in a cross-group cycle — including a clean single
/// workspace whose modules form an internal dependency chain — is returned
/// byte-for-byte unchanged and stays one collapsed unit, preserving the `cargo
/// check -p a -p b …` batching. Every `PerModule` id is likewise unchanged.
///
/// `WholeWorkspace` bases are **never** split: a whole-workspace task is one
/// invocation covering the entire workspace, so its members stay collapsed into
/// a single unit (scheduled at their latest wave). A genuine facade cycle
/// between whole-workspace units is therefore irreducible; rather than failing,
/// [`level_units_into_waves`] condenses the strongly-connected units and
/// co-schedules them into one wave.
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
/// base-group graph. Kept edges are condensed onto distinct base ids
/// (intra-group self-loops dropped); a base is cyclic iff it reaches itself in
/// that self-loop- free graph — any such cycle spans at least two distinct
/// bases. Only these bases are split by layer; a clean single-workspace batch
/// (even one with an internal dependency chain) yields only self-loops and so
/// stays one collapsed unit.
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

/// A base group id, tagged with its layer only when the base spans several
/// layers.
///
/// The layer tag uses the reserved double-`~` marker (`~~L{layer}`) so it can
/// never collide with a `~`-folded group override identity: `~` is rejected in
/// group and member names at the config boundary, so no user identity contains
/// `~`, and a `~~` sequence is therefore unique to this marker. The tag is
/// inserted before the `#task` suffix so the task name stays the final segment;
/// a single-layer base is returned unchanged.
fn layered_id(base: &str, layer: usize, multi_layer: bool) -> String {
    if !multi_layer {
        return base.to_string();
    }
    base.rsplit_once('#').map_or_else(
        || format!("{base}~~L{layer}"),
        |(prefix, task)| format!("{prefix}~~L{layer}#{task}"),
    )
}

/// Fail closed if two *distinct* base groups collapsed onto the same display
/// id, which would silently merge separate units under gating. Modules sharing
/// one base id and layer are the batching we intend; a collision across
/// differing base ids is the pathological case this tripwire rejects.
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
/// Order is the first-seen order across `members`; a `BTreeSet` guards
/// membership so de-duplication stays linear rather than quadratic in the edge
/// count.
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
/// Each unit is placed exactly one wave after its latest `depends_on`
/// dependency (longest-path leveling over the unit edges), so APPLY's strict
/// wave-by-wave execution never submits a unit before a unit it gates on.
/// Deriving waves from the collapsed **unit** graph — rather than from member
/// module wave-indices — is what keeps an un-split multi-layer batch (pulled to
/// a later wave than any single member's module layer) from inverting an
/// external dependent.
///
/// Strongly-connected components of the unit graph are condensed before
/// leveling ([`strongly_connected_components`], [`level_components`]): an
/// acyclic graph has only singleton components, so its waves are byte-identical
/// to a plain longest-path level. A **non-trivial** component (a genuine cycle
/// of two or more units) is co-scheduled into one wave **only** when every one
/// of its units is a whole-workspace invocation whose tool resolves its own
/// cross-workspace dependency closure ([`cycle_co_schedulable`]) — the facade
/// back-dependency shape, where each unit has no real build handoff to another
/// unit. A residual cycle touching any other unit (a `PerModule` unit, a
/// `Batchable` base a layer split could not break, or even a whole-workspace
/// task without the verified closure capability) may encode a real intra-cycle
/// handoff that co-scheduling would silently violate, so it stays a hard typed
/// scheduling error rather than being co-scheduled.
///
/// The mutual `depends_on` edges **inside** a co-scheduled component are then
/// stripped from the units: those peers launch concurrently in one wave, so a
/// surviving intra-cycle gate would let a failing peer block an already
/// in-flight peer and emit a contradictory second terminal outcome for it in
/// APPLY. Cross-component edges (the real handoffs) are preserved untouched.
///
/// [`cycle_co_schedulable`]: PlannedUnit::cycle_co_schedulable
///
/// # Errors
/// A typed internal error if a unit gates on an id absent from `units`, or if a
/// residual cycle contains a unit that is not co-schedulable — both surfaced
/// loudly rather than silently dropped or co-scheduled.
pub(super) fn level_units_into_waves(units: &mut [PlannedUnit]) -> AppResult<Vec<Vec<String>>> {
    let order: Vec<&str> = units.iter().map(|unit| unit.id.as_str()).collect();
    let mut index_of: BTreeMap<&str, usize> = BTreeMap::new();
    for (position, id) in order.iter().enumerate() {
        index_of.insert(*id, position);
    }
    let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); order.len()];
    for (from, unit) in units.iter().enumerate() {
        for dependency in &unit.depends_on {
            let to = *index_of.get(dependency.as_str()).ok_or_else(|| {
                AppError::new(
                    rskit_errors::ErrorCode::Internal,
                    format!("unit '{}' gates on unknown unit '{dependency}'", unit.id),
                )
            })?;
            adjacency[from].push(to);
        }
    }

    let component_of = strongly_connected_components(&adjacency);
    ensure_cycles_co_schedulable(units, &component_of)?;
    let level = level_components(&adjacency, &component_of);
    let depth = level.iter().copied().max().map_or(0, |lvl| lvl + 1);
    let mut waves: Vec<Vec<String>> = vec![Vec::new(); depth];
    for (position, unit) in units.iter().enumerate() {
        waves[level[component_of[position]]].push(unit.id.clone());
    }
    strip_intra_component_edges(units, &component_of);
    waves.retain(|wave| !wave.is_empty());
    Ok(waves)
}

/// Fail closed unless every non-trivial strongly-connected component is
/// composed exclusively of co-schedulable units.
///
/// A component of two or more units is a genuine cycle. Co-scheduling it is
/// only sound when each unit is a whole-workspace invocation whose tool
/// resolves its own cross-workspace dependency closure
/// ([`PlannedUnit::cycle_co_schedulable`]) — then the mutual edges are
/// bookkeeping, not build handoffs. If any unit in the cycle is `PerModule`, an
/// un-splittable `Batchable` base, or a whole-workspace task lacking the
/// verified closure capability, the cycle may encode a real handoff that
/// co-scheduling would silently violate, so it stays the same hard scheduling
/// error a cyclic graph produced before facade co-scheduling existed.
///
/// # Errors
/// A typed internal error naming the cyclic units when a non-trivial component
/// contains a unit that is not co-schedulable.
fn ensure_cycles_co_schedulable(units: &[PlannedUnit], component_of: &[usize]) -> AppResult<()> {
    let component_count = component_of.iter().copied().max().map_or(0, |id| id + 1);
    let mut sizes = vec![0usize; component_count];
    for &component in component_of {
        sizes[component] += 1;
    }
    for (component, size) in sizes.iter().enumerate() {
        if *size < 2 {
            continue;
        }
        let members: Vec<&PlannedUnit> = units
            .iter()
            .zip(component_of)
            .filter(|&(_, &unit_component)| unit_component == component)
            .map(|(unit, _)| unit)
            .collect();
        if members.iter().all(|unit| unit.cycle_co_schedulable) {
            continue;
        }
        let ids: Vec<&str> = members.iter().map(|unit| unit.id.as_str()).collect();
        return Err(AppError::new(
            rskit_errors::ErrorCode::Internal,
            format!(
                "scheduling cycle among units that are not co-schedulable cannot be \
                 co-scheduled: {}",
                ids.join(", ")
            ),
        ));
    }
    Ok(())
}

/// Strip the `depends_on` edges that point **inside** a unit's own
/// strongly-connected component.
///
/// Only non-trivial components (co-scheduled facade cycles) carry such edges.
/// Their units launch concurrently in one wave, so a surviving intra-cycle gate
/// would let a failing peer mark an already in-flight peer `Blocked` and emit a
/// contradictory second terminal outcome for it. Dropping the mutual edges
/// before APPLY leaves each co-scheduled unit gated only on the real
/// cross-component handoffs. A self-loop-free acyclic graph is unaffected: its
/// components are all singletons, so no edge is intra-component.
fn strip_intra_component_edges(units: &mut [PlannedUnit], component_of: &[usize]) {
    let component_by_id: BTreeMap<String, usize> = units
        .iter()
        .zip(component_of)
        .map(|(unit, &component)| (unit.id.clone(), component))
        .collect();
    for (unit, &component) in units.iter_mut().zip(component_of) {
        unit.depends_on.retain(|dependency| {
            component_by_id
                .get(dependency)
                .is_none_or(|&dependency_component| dependency_component != component)
        });
    }
}

/// Assign each unit its strongly-connected-component id via iterative Tarjan
/// over the `depends_on` adjacency (`from` → its dependencies).
///
/// Component ids are handed out in the order components are finalized, which is
/// reverse-topological over the condensation: a component that depends on
/// nothing (a sink of the `depends_on` edges — a dependency root) is finalized
/// first and receives the lowest id. Every dependency component therefore has a
/// strictly lower id than the component that gates on it, which
/// [`level_components`] relies on to level in a single ascending pass. The DFS
/// is iterative (an explicit work stack, no recursion) so a deep unit chain
/// cannot overflow the call stack.
fn strongly_connected_components(adjacency: &[Vec<usize>]) -> Vec<usize> {
    let count = adjacency.len();
    let unvisited = usize::MAX;
    let mut index = vec![unvisited; count];
    let mut lowlink = vec![0usize; count];
    let mut on_stack = vec![false; count];
    let mut scc_stack: Vec<usize> = Vec::new();
    let mut component_of = vec![unvisited; count];
    let mut next_index = 0usize;
    let mut next_component = 0usize;

    for start in 0..count {
        if index[start] != unvisited {
            continue;
        }
        // Each frame is (node, next-child cursor); the cursor makes the DFS resumable
        // after descending into a child, so no recursion is needed.
        let mut work: Vec<(usize, usize)> = vec![(start, 0)];
        while let Some(&(node, cursor)) = work.last() {
            if cursor == 0 {
                index[node] = next_index;
                lowlink[node] = next_index;
                next_index += 1;
                scc_stack.push(node);
                on_stack[node] = true;
            }
            if let Some(&child) = adjacency[node].get(cursor) {
                if let Some(frame) = work.last_mut() {
                    frame.1 = cursor + 1;
                }
                if index[child] == unvisited {
                    work.push((child, 0));
                } else if on_stack[child] {
                    lowlink[node] = lowlink[node].min(index[child]);
                }
                continue;
            }
            // Every child of `node` is explored: if it roots an SCC, pop the component.
            if lowlink[node] == index[node] {
                while let Some(member) = scc_stack.pop() {
                    on_stack[member] = false;
                    component_of[member] = next_component;
                    if member == node {
                        break;
                    }
                }
                next_component += 1;
            }
            work.pop();
            if let Some(&(parent, _)) = work.last() {
                lowlink[parent] = lowlink[parent].min(lowlink[node]);
            }
        }
    }
    component_of
}

/// Longest-path level of each condensed component: a component sits one wave
/// after its latest dependency component.
///
/// Components are keyed by [`strongly_connected_components`] in
/// reverse-topological order, so every dependency component has a strictly
/// lower id and its level is final before the gating component is levelled — a
/// single ascending pass suffices, no topological re-sort. A component with no
/// cross-component dependency lands in wave 0. A cross-component edge inside an
/// SCC cannot exist (both endpoints share the id), so an irreducible cycle
/// simply levels as one component and all its units share a wave.
fn level_components(adjacency: &[Vec<usize>], component_of: &[usize]) -> Vec<usize> {
    let component_count = component_of.iter().copied().max().map_or(0, |id| id + 1);
    let mut members: Vec<Vec<usize>> = vec![Vec::new(); component_count];
    for (node, &component) in component_of.iter().enumerate() {
        members[component].push(node);
    }
    let mut level = vec![0usize; component_count];
    for component in 0..component_count {
        let mut wave = 0usize;
        for &node in &members[component] {
            for &dependency in &adjacency[node] {
                let dependency_component = component_of[dependency];
                if dependency_component != component {
                    wave = wave.max(level[dependency_component] + 1);
                }
            }
        }
        level[component] = wave;
    }
    level
}
