//! The command task table: **only** user-declared `[tasks.*]` argv.
//!
//! The escape-hatch adapter invents no defaults. Every entry in the user's
//! `[ecosystems.command.tasks.*]` table becomes a [`Task`] verbatim — a built-in
//! kind key (`build`, `test`, …) classifies the task by that kind, any other key
//! is a named extra / [`TaskKind::Custom`]. Each entry must define `argv`: there
//! is no default command to inherit (argv-is-sacred).

use std::collections::BTreeMap;

use rskit_errors::{AppError, AppResult};
use toven_ports::{FanOut, RunStrategy, Task, TaskKind, TaskOrigin, TaskOverride};

/// The per-kind default wave-ordering policy.
///
/// Declared `depends_on` edges are honored by default (`LeafToTop`);
/// `format`/`lint` are independent and collapse into one wave. The user
/// overrides via `run_strategy`.
#[must_use]
pub(crate) const fn default_run_strategy(kind: &TaskKind) -> RunStrategy {
    match kind {
        TaskKind::Format | TaskKind::Lint => RunStrategy::Unordered,
        _ => RunStrategy::LeafToTop,
    }
}

/// Resolve the declared tasks, in `overrides` key order.
///
/// # Errors
/// Every declared task must define `argv` — the command adapter has no default
/// to inherit, so an entry without `argv` is rejected.
pub(crate) fn resolve_tasks(overrides: &BTreeMap<String, TaskOverride>) -> AppResult<Vec<Task>> {
    overrides
        .iter()
        .map(|(key, over)| declared_task(key, over))
        .collect()
}

/// Build one declared task from its config entry.
fn declared_task(key: &str, over: &TaskOverride) -> AppResult<Task> {
    let argv = over.argv.clone().ok_or_else(|| {
        AppError::invalid_input(
            format!("ecosystems.command.tasks.{key}"),
            "a command task must define 'argv' (the adapter infers no default command)",
        )
    })?;

    // A built-in kind key classifies the task by that kind (name stays unset);
    // any other key is a named extra carrying the key as its name.
    let (kind, name) = TaskKind::builtin(key).map_or_else(
        || {
            over.kind.as_ref().map_or_else(
                || (TaskKind::Custom(key.to_string()), Some(key.to_string())),
                |kind| (kind.clone(), Some(key.to_string())),
            )
        },
        |kind| (kind, None),
    );

    let mut task = Task::new(kind, argv, over.fan_out.unwrap_or(FanOut::PerModule));
    task.name = name;
    task.origin = TaskOrigin::Project;
    task.selector = over.selector.clone().unwrap_or_default();
    task.shared_inputs.clone_from(&over.shared_inputs);
    task.cache_args = over.cache_args.unwrap_or(false);
    if let Some(persistent) = over.persistent {
        task.persistent = persistent;
    }
    if let Some(readiness) = &over.readiness {
        task.readiness = readiness.clone();
    }
    if let Some(secs) = over.readiness_timeout_secs {
        task.readiness_timeout = std::time::Duration::from_secs(secs);
    }
    Ok(task)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use toven_ports::{FanOut, RunStrategy, TaskKind, TaskOrigin, TaskOverride};

    use super::{default_run_strategy, resolve_tasks};

    fn over(argv: &[&str]) -> TaskOverride {
        TaskOverride {
            argv: Some(argv.iter().map(ToString::to_string).collect()),
            ..TaskOverride::default()
        }
    }

    #[test]
    fn no_declared_tasks_yields_empty_table() {
        let tasks = resolve_tasks(&BTreeMap::new()).expect("resolves");
        assert!(tasks.is_empty());
    }

    #[test]
    fn builtin_key_classifies_by_kind_without_a_name() {
        let mut overrides = BTreeMap::new();
        overrides.insert("build".to_string(), over(&["make", "build"]));
        let tasks = resolve_tasks(&overrides).expect("resolves");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].kind, TaskKind::Build);
        assert!(tasks[0].name.is_none());
        assert_eq!(tasks[0].argv, ["make", "build"]);
        assert_eq!(tasks[0].origin, TaskOrigin::Project);
        assert_eq!(tasks[0].fan_out, FanOut::PerModule);
    }

    #[test]
    fn custom_key_becomes_a_named_custom_task() {
        let mut overrides = BTreeMap::new();
        overrides.insert("deploy".to_string(), over(&["./deploy.sh"]));
        let tasks = resolve_tasks(&overrides).expect("resolves");
        assert_eq!(tasks[0].kind, TaskKind::Custom("deploy".to_string()));
        assert_eq!(tasks[0].name.as_deref(), Some("deploy"));
    }

    #[test]
    fn task_without_argv_is_rejected() {
        let mut overrides = BTreeMap::new();
        overrides.insert("build".to_string(), TaskOverride::default());
        let error = resolve_tasks(&overrides).expect_err("missing argv rejected");
        assert!(error.to_string().contains("argv"), "{error}");
    }

    #[test]
    fn run_strategy_defaults_by_kind() {
        assert_eq!(
            default_run_strategy(&TaskKind::Build),
            RunStrategy::LeafToTop
        );
        assert_eq!(
            default_run_strategy(&TaskKind::Format),
            RunStrategy::Unordered
        );
    }
}
