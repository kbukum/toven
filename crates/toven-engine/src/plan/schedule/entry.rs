//! The [`schedule`] driver: assemble the active module set into waves of rendered
//! units by composing [`ordering`](super::ordering), [`task`](super::task),
//! [`grouping`](super::grouping), and [`unit`](super::unit).

use std::collections::{BTreeMap, BTreeSet};

use rskit_errors::{AppError, AppResult};
use toven_model::{ModuleKey, ToolchainTag, WorkspaceId};

use super::grouping::{group_id_map, layered_group_ids, level_units_into_waves};
use super::ordering::{
    active_modules, active_subgraph, keep_edge, kept_dependencies, layer_index, strategies,
    workspace_index,
};
use super::task::{EffectiveTask, effective_tasks};
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
    // A `run`-kind task can only launch modules with an executable target; drop
    // library-only modules from the schedule so `run` is offered where valid
    // rather than failing at exec on a crate with no binary.
    let (active_modules, effective) = retain_runnable(active_modules, effective);
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
    let mut group_order: Vec<String> = Vec::new();
    for wave in &waves {
        for reference in wave {
            let id = group_ids.get(reference).ok_or_else(|| {
                AppError::new(
                    rskit_errors::ErrorCode::Internal,
                    format!("scheduled unknown module '{reference}'"),
                )
            })?;
            if !group_members.contains_key(id) {
                group_order.push(id.clone());
            }
            group_members
                .entry(id.clone())
                .or_default()
                .push(reference.clone());
        }
    }
    for id in &group_order {
        let members = &group_members[id];
        let unit = plan_unit(
            request,
            id,
            members,
            &active_modules,
            &effective,
            toolchain,
            &workspaces,
            &kept_deps,
            &group_ids,
        )?;
        units.push(unit);
    }

    // Level waves from the condensed unit graph (`depends_on`), not from member
    // module wave-indices: an un-split multi-layer batch is pulled to the wave
    // after its latest dependency, so APPLY never runs a dependent before it.
    let wave_ids = level_units_into_waves(&units)?;

    Ok(Scheduled {
        units,
        waves: wave_ids,
    })
}

/// Drop modules whose resolved task is [`TaskKind::Run`](toven_ports::TaskKind::Run)
/// but which expose no executable target ([`Module::runnable`] is `false`).
///
/// A persistent `run` on a library-only crate is invalid — `cargo run` has no
/// binary to launch — so the schedule excludes it rather than emitting a unit
/// that fails at exec. Non-`Run` tasks (build/test/lint/…) are never filtered:
/// every active module keeps its unit.
fn retain_runnable(
    active_modules: BTreeMap<ModuleKey, toven_model::Module>,
    effective: BTreeMap<ModuleKey, EffectiveTask>,
) -> (
    BTreeMap<ModuleKey, toven_model::Module>,
    BTreeMap<ModuleKey, EffectiveTask>,
) {
    use toven_ports::TaskKind;

    let dropped: BTreeSet<ModuleKey> = effective
        .iter()
        .filter(|(key, effective)| {
            effective.task.kind == TaskKind::Run
                && active_modules
                    .get(*key)
                    .is_some_and(|module| !module.runnable)
        })
        .map(|(key, _)| key.clone())
        .collect();

    if dropped.is_empty() {
        return (active_modules, effective);
    }
    let active_modules = active_modules
        .into_iter()
        .filter(|(key, _)| !dropped.contains(key))
        .collect();
    let effective = effective
        .into_iter()
        .filter(|(key, _)| !dropped.contains(key))
        .collect();
    (active_modules, effective)
}
