//! The [`schedule`] driver: assemble the active module set into waves of rendered
//! units by composing [`ordering`](super::ordering), [`task`](super::task),
//! [`grouping`](super::grouping), and [`unit`](super::unit).

use std::collections::BTreeMap;

use rskit_errors::{AppError, AppResult};
use toven_model::{ModuleKey, ToolchainTag, WorkspaceId};

use super::grouping::{ensure_condensed_acyclic, group_id_map, layered_group_ids};
use super::ordering::{
    active_modules, active_subgraph, keep_edge, kept_dependencies, layer_index, strategies,
    workspace_index,
};
use super::task::effective_tasks;
use super::unit::{PlannedUnit, plan_unit};
use crate::plan::configure::MemberAdapters;
use crate::plan::discover::Federation;
use crate::plan::overrides::GroupOverrides;
use crate::plan::request::PlanRequest;

/// The scheduled units plus the wave-ordered unit ids.
#[derive(Debug, Clone)]
pub(in crate::plan) struct Scheduled {
    /// All planned units, wave order independent.
    pub(in crate::plan) units: Vec<PlannedUnit>,
    /// Wave-ordered unit ids (each inner vec is one ready wave).
    pub(in crate::plan) waves: Vec<Vec<String>>,
}

/// Schedule the active module set into waves of rendered units.
///
/// `overrides` carries any `[groups.*]` scope overrides: a group's `run_strategy`
/// wins over the ecosystem default for its members, and a group's `tasks` entry
/// field-merges onto the ecosystem/adapter default for the intent (marking the
/// resolved task [`TaskOrigin::Group`](toven_ports::TaskOrigin::Group)). An
/// overridden batch unit is kept distinct from the un-overridden default so
/// members never collapse across differing argv.
///
/// # Errors
/// An active module with no configured adapter or no task for the intent, a
/// missing workspace a template requires, or a template parse/render failure.
pub(in crate::plan) fn schedule(
    request: &PlanRequest,
    federation: &Federation,
    active: &[ModuleKey],
    adapters: &MemberAdapters,
    overrides: &GroupOverrides,
    toolchain: &BTreeMap<WorkspaceId, ToolchainTag>,
) -> AppResult<Scheduled> {
    let active_modules = active_modules(federation, active);
    let effective = effective_tasks(&active_modules, adapters, &request.intent, overrides)?;
    let strategies = strategies(&active_modules, adapters, overrides, &effective)?;
    let subgraph = active_subgraph(&active_modules, federation)?;

    let waves = subgraph.waves(|edge| keep_edge(edge, &strategies))?;
    let kept_deps = kept_dependencies(&active_modules, federation, &strategies);

    let workspaces = workspace_index(federation);
    let base_ids = group_id_map(&active_modules, &effective)?;
    let layer_of = layer_index(&waves);
    let group_ids = layered_group_ids(&base_ids, &layer_of, &effective, &kept_deps)?;

    let mut units = Vec::new();
    let mut group_members: BTreeMap<String, Vec<ModuleKey>> = BTreeMap::new();
    let mut group_wave: BTreeMap<String, usize> = BTreeMap::new();
    let mut wave_order: Vec<String> = Vec::new();
    for (index, wave) in waves.iter().enumerate() {
        for reference in wave {
            let id = group_ids.get(reference).ok_or_else(|| {
                AppError::new(
                    rskit_errors::ErrorCode::Internal,
                    format!("scheduled unknown module '{reference}'"),
                )
            })?;
            if !group_members.contains_key(id) {
                wave_order.push(id.clone());
            }
            group_members
                .entry(id.clone())
                .or_default()
                .push(reference.clone());
            // A layer-split group (a cyclic base broken per layer) is layer-
            // homogeneous and occupies one wave; a whole-workspace group and any
            // un-split batch spanning layers take the latest wave any member
            // occupies, after which their dependencies have all run.
            // `ensure_condensed_acyclic` guards the result.
            group_wave
                .entry(id.clone())
                .and_modify(|w| *w = (*w).max(index))
                .or_insert(index);
        }
    }
    let last_wave = group_wave.values().copied().max().map_or(0, |w| w + 1);
    let mut wave_ids: Vec<Vec<String>> = vec![Vec::new(); last_wave];
    for id in wave_order {
        let members = &group_members[&id];
        let unit = plan_unit(
            request,
            &id,
            members,
            &active_modules,
            &effective,
            toolchain,
            &workspaces,
            &kept_deps,
            &group_ids,
        )?;
        wave_ids[group_wave[&id]].push(unit.id.clone());
        units.push(unit);
    }
    wave_ids.retain(|wave| !wave.is_empty());

    ensure_condensed_acyclic(&units)?;

    Ok(Scheduled {
        units,
        waves: wave_ids,
    })
}
