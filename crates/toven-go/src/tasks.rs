//! The default `go` task table and user-override resolution.
//!
//! The adapter ships one default [`Task`] per built-in [`TaskKind`]. Go commands
//! are scoped to a module with `go -C {module.root} <subcommand>` and fan out
//! per module (`go` has no native multi-module batch), with the package pattern
//! `./...` carried in the per-module `selector`. [`resolve_tasks`] field-merges
//! the user's `[ecosystems.go.tasks.*]` overrides onto those defaults and
//! appends named/custom extras, so [`default_tasks`] hands the engine the fully
//! resolved set.

use std::collections::BTreeMap;

use rskit_errors::{AppError, AppResult};
use toven_ports::{FanOut, RunStrategy, Task, TaskKind, TaskOrigin, TaskOverride, merge_task};

/// The workspace-level cache input every `go` task shares: the checksum file
/// pins resolved dependency versions, so a change invalidates the workspace.
const SHARED_SUM: &str = "go.sum";

/// The package pattern a per-module `go` invocation targets (all packages in the
/// module), spliced at the explicit `{module.selector}` point.
const PACKAGE_PATTERN: &str = "./...";

/// Build the adapter's built-in default task for `kind`, before any user
/// override is applied.
fn default_task(kind: &TaskKind) -> Task {
    match kind {
        TaskKind::Format => module_task("format", "fmt"),
        TaskKind::Build => module_task("build", "build"),
        // Go ships no separate type-check / lint; `go vet` is the closest
        // built-in default. Users swap in `golangci-lint` via a task override.
        TaskKind::Check => module_task("check", "vet"),
        TaskKind::Lint => module_task("lint", "vet"),
        TaskKind::Test => module_task("test", "test"),
        TaskKind::Doc => doc_task(),
        TaskKind::Run => run_task(),
        TaskKind::Custom(name) => module_task(name, name),
    }
}

/// A `go` task scoped to one module via `go -C {module.root} <subcommand>
/// {module.selector}`, fanning out per module with the `./...` package pattern.
fn module_task(kind_name: &str, subcommand: &str) -> Task {
    let argv = vec![
        "go".to_string(),
        "-C".to_string(),
        "{module.root}".to_string(),
        subcommand.to_string(),
        "{module.selector}".to_string(),
        "{args}".to_string(),
    ];
    let mut task = Task::new(kind_for(kind_name), argv, FanOut::PerModule);
    task.selector = vec![PACKAGE_PATTERN.to_string()];
    task.shared_inputs = vec![SHARED_SUM.to_string()];
    task
}

/// `go doc` prints documentation for a single package and takes no `./...`
/// pattern, so the default targets the module root without a selector.
fn doc_task() -> Task {
    let argv = vec![
        "go".to_string(),
        "-C".to_string(),
        "{module.root}".to_string(),
        "doc".to_string(),
        "{args}".to_string(),
    ];
    let mut task = Task::new(TaskKind::Doc, argv, FanOut::PerModule);
    task.shared_inputs = vec![SHARED_SUM.to_string()];
    task
}

/// `go run` builds and runs the module's main package; the default is
/// persistent (servers / long-running mains), mirroring the Rust `run` default.
fn run_task() -> Task {
    let argv = vec![
        "go".to_string(),
        "-C".to_string(),
        "{module.root}".to_string(),
        "run".to_string(),
        ".".to_string(),
        "{args}".to_string(),
    ];
    let mut task = Task::new(TaskKind::Run, argv, FanOut::PerModule);
    task.persistent = true;
    task.shared_inputs = vec![SHARED_SUM.to_string()];
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
            format!("ecosystems.go.tasks.{key}"),
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
    fn test_default_fans_out_per_module_with_sum_input() {
        let tasks = resolve_tasks(&BTreeMap::new()).expect("defaults resolve");
        let test = tasks
            .iter()
            .find(|task| task.kind == TaskKind::Test)
            .expect("test task present");
        assert_eq!(test.fan_out, FanOut::PerModule);
        assert_eq!(test.selector, ["./..."]);
        assert_eq!(test.shared_inputs, ["go.sum"]);
        assert_eq!(test.argv.first().map(String::as_str), Some("go"));
        assert!(test.argv.contains(&"{module.root}".to_string()));
        assert!(test.argv.contains(&"{module.selector}".to_string()));
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
                argv: Some(vec![
                    "gotestsum".into(),
                    "--".into(),
                    "{module.selector}".into(),
                ]),
                cache_args: Some(true),
                ..TaskOverride::default()
            },
        );
        let tasks = resolve_tasks(&overrides).expect("resolves");
        let test = tasks
            .iter()
            .find(|task| task.kind == TaskKind::Test)
            .expect("test present");
        assert_eq!(test.argv, ["gotestsum", "--", "{module.selector}"]);
        assert!(test.cache_args);
        // selector + shared_inputs inherited from the default:
        assert_eq!(test.selector, ["./..."]);
        assert_eq!(test.shared_inputs, ["go.sum"]);
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
