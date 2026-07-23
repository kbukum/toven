//! Resolve each active module's effective task for the plan intent.
//!
//! The config `tasks` table is the single source of runnable tasks. For each
//! module the adapter default is selected by the intent's addressable name,
//! then field-merged with any `[groups.*].tasks` override, yielding an
//! [`EffectiveTask`] the scheduler consumes for ordering, grouping, and
//! rendering.

use std::collections::BTreeMap;

use rskit_errors::{AppError, AppResult};
use toven_model::{Module, ModuleKey};
use toven_ports::{Task, TaskIntent, TaskOrigin, merge_task};

use crate::plan::configure::MemberAdapters;
use crate::plan::overrides::GroupOverrides;

/// A module's resolved task for the intent, with the group (if any) whose
/// override produced it.
pub(super) struct EffectiveTask {
    /// The adapter default field-merged with any group override.
    pub(super) task: Task,
    /// The declaring group when a `[groups.*].tasks` override applied.
    pub(super) group: Option<String>,
}

/// Resolve every active module's effective task for the intent: the adapter
/// default, field-merged with the module's group task override when one
/// applies.
pub(super) fn effective_tasks(
    modules: &BTreeMap<ModuleKey, Module>,
    adapters: &MemberAdapters,
    intent: &TaskIntent,
    overrides: &GroupOverrides,
) -> AppResult<BTreeMap<ModuleKey, EffectiveTask>> {
    let mut resolved = BTreeMap::new();
    for (key, module) in modules {
        let adapter = adapter_for(module, adapters)?;
        let ecosystem = module.id.ecosystem.as_str();
        let config_tasks = config_tasks(adapter, ecosystem)?;
        let default = select_task(&config_tasks, intent)
            .ok_or_else(|| unknown_task_error(ecosystem, intent, &config_tasks))?;
        let effective = match overrides.task(key, intent.name()) {
            Some((group, over)) => {
                let mut merged = merge_task(&default, over);
                merged.origin = TaskOrigin::Group;
                EffectiveTask {
                    task: merged,
                    group: Some(group.to_string()),
                }
            }
            None => EffectiveTask {
                task: default,
                group: None,
            },
        };
        resolved.insert(key.clone(), effective);
    }
    Ok(resolved)
}

/// Look up a module's resolved effective task, failing closed on an unknown
/// key.
pub(super) fn effective_for<'a>(
    key: &ModuleKey,
    effective: &'a BTreeMap<ModuleKey, EffectiveTask>,
) -> AppResult<&'a EffectiveTask> {
    effective.get(key).ok_or_else(|| {
        AppError::new(
            rskit_errors::ErrorCode::Internal,
            format!("scheduled unknown module '{key}'"),
        )
    })
}

/// Materialize an ecosystem's authoritative config task table into resolved
/// [`Task`]s (the `tasks` map exposed via
/// [`ConfiguredAdapter::common`](toven_ports::ConfiguredAdapter::common)).
///
/// The config is the single source of runnable tasks: an entry with an empty
/// `argv` fails here with a typed error citing its
/// `ecosystems.<id>.tasks.<name>` path (the same completeness check `configure`
/// runs).
fn config_tasks(
    adapter: &dyn toven_ports::ConfiguredAdapter,
    ecosystem: &str,
) -> AppResult<Vec<Task>> {
    adapter
        .common()
        .tasks
        .iter()
        .map(|(key, entry)| entry.materialize(ecosystem, key))
        .collect()
}

/// Build the typed error for an intent that no config task in `ecosystem`
/// satisfies. When the ecosystem has no task table at all, point the user at
/// `toven init` to author one; otherwise enrich the error with the nearest
/// resolvable task name and the `toven tasks` discovery hint.
///
/// The candidate set is the ecosystem's config task names (canonical, so
/// `format` not `fmt`); a nearest match within the default edit-distance is
/// offered as advisory data in the message. The error stays a typed
/// [`AppError`] — the CLI's renderer is what prints it.
pub(super) fn unknown_task_error(
    ecosystem: &str,
    intent: &TaskIntent,
    available: &[Task],
) -> AppError {
    let wanted = intent.name();
    // Built with `AppError::new` rather than `invalid_input` so the rendered line
    // reads as one sentence (`error[INVALID_INPUT]: ecosystem 'rust' has no …`)
    // instead of stuttering an `invalid tasks:` field prefix in front of a message
    // that is already about tasks.
    if available.is_empty() {
        return AppError::new(
            rskit_errors::ErrorCode::InvalidInput,
            format!(
                "ecosystem '{ecosystem}' defines no tasks. Run 'toven init' to author its task table in toven.toml."
            ),
        );
    }
    let names: Vec<String> = available.iter().map(task_addressable_name).collect();
    let suggestion = rskit_util::strings::nearest(wanted, names.iter().map(String::as_str))
        .map_or_else(String::new, |name| format!(" Did you mean '{name}'?"));
    AppError::new(
        rskit_errors::ErrorCode::InvalidInput,
        format!(
            "ecosystem '{ecosystem}' has no '{wanted}' task.{suggestion} Run 'toven tasks' to list every runnable task."
        ),
    )
}

/// The user-addressable canonical name of a resolved task — its identity.
fn task_addressable_name(task: &Task) -> String {
    task.name.clone()
}

/// Select the config task a user token resolves to by its addressable name.
///
/// A task's addressable identity is its name (the table key). `intent.name()`
/// is the exact token the user typed, so a direct name match resolves both a
/// plain built-in (`test` → the `test` task) and a renamed/extra task
/// (`my-test` → the `kind = "test"` entry) without collision.
fn select_task(tasks: &[Task], intent: &TaskIntent) -> Option<Task> {
    let wanted = intent.name();
    tasks
        .iter()
        .find(|task| task_addressable_name(task) == wanted)
        .cloned()
}

/// Look up the configured adapter that owns a module within its member.
pub(super) fn adapter_for<'a>(
    module: &Module,
    adapters: &'a MemberAdapters,
) -> AppResult<&'a dyn toven_ports::ConfiguredAdapter> {
    adapters
        .get(module.member.as_ref(), &module.id.ecosystem)
        .ok_or_else(|| {
            AppError::new(
                rskit_errors::ErrorCode::Internal,
                format!(
                    "no configured adapter for ecosystem '{}'",
                    module.id.ecosystem
                ),
            )
        })
}
