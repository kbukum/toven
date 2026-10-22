//! The [`schedule`] driver: assemble the active module set into waves of
//! rendered units by composing [`ordering`](super::ordering),
//! [`task`](super::task), [`grouping`](super::grouping), and
//! [`unit`](super::unit).

use std::collections::{BTreeMap, BTreeSet};

use rskit_errors::{AppError, AppResult};
use toven_model::{Module, ModuleKey, SkipReason, SkippedModule, ToolchainTag, WorkspaceId};

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
    /// Active modules left out for missing required capabilities.
    pub(in crate::plan) skipped: Vec<SkippedModule>,
}

/// Schedule the active module set into waves of rendered units.
///
/// `overrides` carries any `[groups.*]` scope overrides: a group's
/// `run_strategy` wins over the ecosystem default for its members, and a
/// group's `tasks` entry field-merges onto the ecosystem/adapter default for
/// the intent (marking the resolved task
/// [`TaskOrigin::Group`](toven_ports::TaskOrigin::Group)). An overridden batch
/// unit is kept distinct from the un-overridden default so members never
/// collapse across differing argv.
///
/// Active modules that lack a capability the effective task `requires` are
/// left out and reported as [`SkippedModule`]s; a module in `named` (one the
/// user selected by exact name) is rejected instead, so an explicit request
/// never silently runs nothing.
///
/// # Errors
/// An active module with no configured adapter or no task for the intent, a
/// named module that lacks a required capability, a missing workspace a
/// template requires, or a template parse/render failure.
#[allow(clippy::too_many_arguments)]
pub(in crate::plan) fn schedule(
    request: &PlanRequest,
    federation: &Federation,
    active: &[ModuleKey],
    named: &BTreeSet<ModuleKey>,
    adapters: &MemberAdapters,
    overrides: &GroupOverrides,
    toolchain: &BTreeMap<WorkspaceId, ToolchainTag>,
) -> AppResult<Scheduled> {
    let active_modules = active_modules(federation, active);
    let effective = effective_tasks(&active_modules, adapters, &request.intent, overrides)?;
    let Eligible {
        modules: active_modules,
        effective,
        skipped,
    } = retain_eligible(active_modules, effective, named, request.intent.name())?;
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
    let wave_ids = level_units_into_waves(&mut units)?;

    Ok(Scheduled {
        units,
        waves: wave_ids,
        skipped,
    })
}

/// The `active` modules eligible for the intent's effective task (named by its
/// `modules` list when set, providing every capability it `requires`).
///
/// Uses the same rule as [`schedule`], so prerequisite expansion starts only
/// from modules that will actually run the requested task.
///
/// # Errors
/// An active module with no configured adapter or no task for the intent.
pub(in crate::plan) fn eligible_modules(
    federation: &Federation,
    active: &BTreeSet<ModuleKey>,
    adapters: &MemberAdapters,
    intent: &toven_ports::TaskIntent,
    overrides: &GroupOverrides,
) -> AppResult<BTreeSet<ModuleKey>> {
    let keys: Vec<ModuleKey> = active.iter().cloned().collect();
    let modules = active_modules(federation, &keys);
    let effective = effective_tasks(&modules, adapters, intent, overrides)?;
    Ok(effective
        .iter()
        .filter(|(key, resolved)| {
            modules
                .get(*key)
                .is_some_and(|module| ineligibility(module, &resolved.task).is_none())
        })
        .map(|(key, _)| key.clone())
        .collect())
}

/// The active modules and effective tasks eligible for the intent, plus the
/// ones left out.
struct Eligible {
    modules: BTreeMap<ModuleKey, Module>,
    effective: BTreeMap<ModuleKey, EffectiveTask>,
    skipped: Vec<SkippedModule>,
}

/// Keep only modules eligible for their effective task: named by its `modules`
/// list (when set) and providing every capability it `requires`.
///
/// An ineligible module selected broadly (all, affected, glob, workspace, or a
/// dependency/dependents closure) is skipped and reported. One the user named
/// exactly is an error saying why.
fn retain_eligible(
    mut modules: BTreeMap<ModuleKey, Module>,
    mut effective: BTreeMap<ModuleKey, EffectiveTask>,
    named: &BTreeSet<ModuleKey>,
    task: &str,
) -> AppResult<Eligible> {
    let mut skipped = Vec::new();
    for (key, resolved) in &effective {
        let Some(module) = modules.get(key) else {
            continue;
        };
        let Some(reason) = ineligibility(module, &resolved.task) else {
            continue;
        };
        if named.contains(key) {
            return Err(ineligible_error(key, task, &resolved.task.modules, &reason));
        }
        skipped.push(SkippedModule {
            module: key.clone(),
            task: task.to_string(),
            reason,
        });
    }
    for skip in &skipped {
        modules.remove(&skip.module);
        effective.remove(&skip.module);
    }
    Ok(Eligible {
        modules,
        effective,
        skipped,
    })
}

/// Why `module` cannot run `task`, or `None` when it is eligible.
fn ineligibility(module: &Module, task: &toven_ports::Task) -> Option<SkipReason> {
    if !task.modules.is_empty() && !task.modules.contains(&module.id.name) {
        return Some(SkipReason::NotListed);
    }
    let missing: Vec<_> = task
        .requires
        .iter()
        .filter(|capability| !module.capabilities.contains(*capability))
        .cloned()
        .collect();
    (!missing.is_empty()).then_some(SkipReason::MissingCapabilities { missing })
}

/// The typed error for an exactly named module the task cannot run on.
fn ineligible_error(
    key: &ModuleKey,
    task: &str,
    listed: &[String],
    reason: &SkipReason,
) -> AppError {
    let detail = match reason {
        SkipReason::MissingCapabilities { missing } => {
            let missing = missing
                .iter()
                .map(toven_model::Capability::as_str)
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "it does not provide {missing}. Select a module that does, or change the task's `requires`."
            )
        }
        SkipReason::NotListed => format!(
            "the task applies only to {}. Select one of them, or change the task's `modules`.",
            listed.join(", ")
        ),
        _ => "it is not eligible.".to_string(),
    };
    AppError::new(
        rskit_errors::ErrorCode::InvalidInput,
        format!("module '{key}' cannot run task '{task}': {detail}"),
    )
}
