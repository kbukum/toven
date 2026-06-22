//! The default cargo task table and user-override resolution.
//!
//! The adapter ships one default [`Task`] per built-in [`TaskKind`] with a
//! two-template `argv` + `selector`, a per-kind [`FanOut`] ceiling, and the
//! workspace-level `shared_inputs = ["Cargo.lock"]`. [`resolve_tasks`] field-
//! merges the user's `[ecosystems.rust.tasks.*]` overrides onto those defaults
//! and appends named/custom extras, so [`default_tasks`] hands the engine the
//! fully resolved set.

use std::collections::BTreeMap;

use rskit_errors::{AppError, AppResult};
use toven_ports::{FanOut, RunStrategy, Task, TaskKind, TaskOrigin, TaskOverride, merge_task};

/// The workspace-level cache input every cargo task shares: the lockfile pins the
/// resolved dependency versions, so a change invalidates every task in the
/// workspace.
const SHARED_LOCKFILE: &str = "Cargo.lock";

/// Build the adapter's built-in default task for `kind`, before any user
/// override is applied.
fn default_task(kind: &TaskKind) -> Task {
    match kind {
        TaskKind::Format => whole_workspace("fmt"),
        TaskKind::Run => {
            let mut task = fan_out_task("run", "run", FanOut::PerModule);
            task.persistent = true;
            task
        }
        TaskKind::Build => fan_out_task("build", "build", FanOut::Batchable),
        TaskKind::Check => fan_out_task("check", "check", FanOut::Batchable),
        TaskKind::Lint => fan_out_task("lint", "clippy", FanOut::Batchable),
        TaskKind::Test => fan_out_task("test", "test", FanOut::Batchable),
        TaskKind::Doc => fan_out_task("doc", "doc", FanOut::Batchable),
        TaskKind::Custom(name) => fan_out_task(name, name, FanOut::PerModule),
    }
}

/// A cargo task that fans out over modules via `-p {module.package}`, spliced at
/// the explicit `{module.selector}` point.
fn fan_out_task(kind_name: &str, subcommand: &str, fan_out: FanOut) -> Task {
    let argv = vec![
        "cargo".to_string(),
        subcommand.to_string(),
        "--manifest-path".to_string(),
        "{module.manifest}".to_string(),
        "{module.selector}".to_string(),
        "{args}".to_string(),
    ];
    let mut task = Task::new(kind_for(kind_name), argv, fan_out);
    task.selector = vec!["-p".to_string(), "{module.package}".to_string()];
    task.shared_inputs = vec![SHARED_LOCKFILE.to_string()];
    task
}

/// A cargo task that runs once for the whole workspace (no per-module selector),
/// e.g. `cargo fmt --all`.
fn whole_workspace(subcommand: &str) -> Task {
    let argv = vec![
        "cargo".to_string(),
        subcommand.to_string(),
        "--manifest-path".to_string(),
        "{module.manifest}".to_string(),
        "--all".to_string(),
        "{args}".to_string(),
    ];
    let kind_name = if subcommand == "fmt" {
        "format"
    } else {
        subcommand
    };
    let mut task = Task::new(kind_for(kind_name), argv, FanOut::WholeWorkspace);
    task.shared_inputs = vec![SHARED_LOCKFILE.to_string()];
    task
}

/// Resolve a built-in [`TaskKind`] from its canonical name, falling back to
/// [`TaskKind::Custom`] for anything else.
fn kind_for(name: &str) -> TaskKind {
    TaskKind::builtin(name).unwrap_or_else(|| TaskKind::Custom(name.to_string()))
}

/// The built-in kinds the adapter ships a default for, in canonical order.
const BUILTIN_KINDS: [TaskKind; 7] = [
    TaskKind::Build,
    TaskKind::Check,
    TaskKind::Format,
    TaskKind::Lint,
    TaskKind::Test,
    TaskKind::Doc,
    TaskKind::Run,
];

/// The per-kind default wave-ordering policy.
///
/// Compilation-bearing kinds (`build`/`check`/`test`/`doc`/`run`) respect the
/// dependency graph; `format`/`lint` are independent and collapse into one wave.
#[must_use]
pub(crate) const fn default_run_strategy(kind: &TaskKind) -> RunStrategy {
    match kind {
        TaskKind::Format | TaskKind::Lint => RunStrategy::Unordered,
        _ => RunStrategy::LeafToTop,
    }
}

/// The adapter's default task set, field-merged with the user's `tasks`
/// overrides.
///
/// Built-in kinds are emitted in canonical order (overrides field-merged onto
/// each default); named extras and custom tasks follow in `overrides` key order.
///
/// # Errors
/// A named extra that has no matching built-in default must supply `argv` — its
/// command cannot be inherited, so an extra without `argv` is rejected.
pub(crate) fn resolve_tasks(overrides: &BTreeMap<String, TaskOverride>) -> AppResult<Vec<Task>> {
    let mut builtins: Vec<Task> = BUILTIN_KINDS.iter().map(default_task).collect();

    for (key, over) in overrides {
        if let Some(kind) = TaskKind::builtin(key)
            && let Some(slot) = builtins.iter_mut().find(|task| task.kind == kind)
        {
            *slot = merge_task(slot, over);
        }
    }

    let mut tasks = builtins;
    for (key, over) in overrides {
        if TaskKind::builtin(key).is_none() {
            tasks.push(named_extra(key, over)?);
        }
    }
    Ok(tasks)
}

/// Build a named extra / custom task from its override (nothing to merge from).
fn named_extra(key: &str, over: &TaskOverride) -> AppResult<Task> {
    let argv = over.argv.clone().ok_or_else(|| {
        AppError::invalid_input(
            format!("ecosystems.rust.tasks.{key}"),
            "a named task with no matching built-in default must define 'argv'",
        )
    })?;

    let (kind, name) = over.kind.as_ref().map_or_else(
        || (TaskKind::Custom(key.to_string()), None),
        |kind| (kind.clone(), Some(key.to_string())),
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

    #[test]
    fn default_table_covers_every_builtin_kind() {
        let tasks = resolve_tasks(&BTreeMap::new()).expect("defaults resolve");
        let kinds: Vec<_> = tasks.iter().map(|task| task.kind.clone()).collect();
        assert_eq!(
            kinds,
            vec![
                TaskKind::Build,
                TaskKind::Check,
                TaskKind::Format,
                TaskKind::Lint,
                TaskKind::Test,
                TaskKind::Doc,
                TaskKind::Run,
            ]
        );
    }

    #[test]
    fn test_default_is_batchable_with_lockfile_input() {
        let tasks = resolve_tasks(&BTreeMap::new()).expect("defaults resolve");
        let test = tasks
            .iter()
            .find(|task| task.kind == TaskKind::Test)
            .expect("test task present");
        assert_eq!(test.fan_out, FanOut::Batchable);
        assert_eq!(test.selector, ["-p", "{module.package}"]);
        assert_eq!(test.shared_inputs, ["Cargo.lock"]);
        assert!(test.argv.contains(&"{module.selector}".to_string()));
    }

    #[test]
    fn format_is_whole_workspace_without_selector() {
        let tasks = resolve_tasks(&BTreeMap::new()).expect("defaults resolve");
        let fmt = tasks
            .iter()
            .find(|task| task.kind == TaskKind::Format)
            .expect("format task present");
        assert_eq!(fmt.fan_out, FanOut::WholeWorkspace);
        assert!(fmt.selector.is_empty());
    }

    #[test]
    fn run_default_is_persistent() {
        let tasks = resolve_tasks(&BTreeMap::new()).expect("defaults resolve");
        let run = tasks
            .iter()
            .find(|task| task.kind == TaskKind::Run)
            .expect("run task present");
        assert!(run.persistent);
        assert_eq!(run.fan_out, FanOut::PerModule);
    }

    #[test]
    fn builtin_override_field_merges_onto_default() {
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "test".to_string(),
            TaskOverride {
                argv: Some(vec!["cargo".into(), "nextest".into(), "run".into()]),
                cache_args: Some(true),
                ..TaskOverride::default()
            },
        );
        let tasks = resolve_tasks(&overrides).expect("resolves");
        let test = tasks
            .iter()
            .find(|task| task.kind == TaskKind::Test)
            .expect("test present");
        assert_eq!(test.argv, ["cargo", "nextest", "run"]);
        assert!(test.cache_args);
        // selector + shared_inputs inherited from the default:
        assert_eq!(test.selector, ["-p", "{module.package}"]);
        assert_eq!(test.shared_inputs, ["Cargo.lock"]);
        assert_eq!(test.origin, TaskOrigin::Project);
    }

    #[test]
    fn named_extra_requires_argv() {
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "test-integration".to_string(),
            TaskOverride {
                kind: Some(TaskKind::Test),
                ..TaskOverride::default()
            },
        );
        let error = resolve_tasks(&overrides).expect_err("missing argv rejected");
        assert!(error.to_string().contains("argv"), "{error}");
    }

    #[test]
    fn named_extra_is_appended_with_name() {
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "test-integration".to_string(),
            TaskOverride {
                kind: Some(TaskKind::Test),
                argv: Some(vec![
                    "cargo".into(),
                    "test".into(),
                    "--test".into(),
                    "it".into(),
                ]),
                ..TaskOverride::default()
            },
        );
        let tasks = resolve_tasks(&overrides).expect("resolves");
        let extra = tasks
            .iter()
            .find(|task| task.name.as_deref() == Some("test-integration"))
            .expect("named extra present");
        assert_eq!(extra.kind, TaskKind::Test);
        assert_eq!(extra.origin, TaskOrigin::Project);
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
        assert_eq!(
            default_run_strategy(&TaskKind::Lint),
            RunStrategy::Unordered
        );
    }
}
