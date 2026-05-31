//! `toven <task>` execution command.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    path::PathBuf,
    time::Duration,
};

use clap::ArgMatches;

use crate::{
    adapter::AdapterRegistry,
    cache::decision::{
        CACHE_DIRECTORY, CacheDecision, CacheDecisions, CacheMode, TaskCache,
        prepare_cache_decisions,
    },
    cli::affected::{modules_from_discovered, resolve_affected_changes, resolve_affected_modules},
    config::load_workspace,
    core::{
        AppError, AppResult, ErrorCode, ExecutionMode, ExecutionUnit, Module, ModuleId, Plan,
        Workspace,
    },
    engine::{
        DiscoveredTaskProfile, discover_workspace_task_profiles, plan_discovered_task_profiles,
    },
    exec::{
        PersistentOutput, PersistentOutputStream, PersistentProcess, RunOptions,
        SharedCancellation, run_execution_unit, spawn_ctrl_c_handler,
        start_persistent_execution_unit_with_output, stop_ctrl_c_handler,
    },
    report::{OutputFormat, RunReporter},
};

pub(super) fn run_task(
    matches: &ArgMatches,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> AppResult<()> {
    if matches.get_flag("watch") {
        return crate::cli::watch::run_watch(matches, stdout, stderr);
    }
    run_task_once(matches, stdout, stderr, None)
}

pub(super) fn run_task_once(
    matches: &ArgMatches,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
    module_filter: Option<&BTreeSet<ModuleId>>,
) -> AppResult<()> {
    run_task_once_with_lifecycle(
        matches,
        stdout,
        stderr,
        module_filter,
        PersistentLifecycle::Block,
        None,
    )
    .map(|_| ())
}

pub(super) fn run_task_once_for_watch(
    matches: &ArgMatches,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
    module_filter: Option<&BTreeSet<ModuleId>>,
    cancellation: SharedCancellation,
) -> AppResult<Vec<ActivePersistentProcess>> {
    run_task_once_with_lifecycle(
        matches,
        stdout,
        stderr,
        module_filter,
        PersistentLifecycle::KeepAlive,
        Some(cancellation),
    )
}

fn run_task_once_with_lifecycle(
    matches: &ArgMatches,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
    module_filter: Option<&BTreeSet<ModuleId>>,
    persistent_lifecycle: PersistentLifecycle,
    cancellation: Option<SharedCancellation>,
) -> AppResult<Vec<ActivePersistentProcess>> {
    let output_format = OutputFormat::parse(
        matches
            .get_one::<String>("output")
            .expect("clap supplies the run output default"),
    )?;
    let config = PathBuf::from(
        matches
            .get_one::<String>("config")
            .expect("clap supplies the run config default"),
    );
    let task = matches
        .get_one::<String>("task")
        .expect("clap requires a run task")
        .as_str();
    let passthrough_args = matches
        .get_many::<String>("args")
        .map(|values| values.cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let workspace = load_workspace(config)?;
    let registry = AdapterRegistry::default();
    let discovered = discover_workspace_task_profiles(&workspace, task, &registry)?;
    let (exec_plan, cache_plan) = select_plans(
        matches,
        &workspace,
        &discovered,
        &passthrough_args,
        module_filter,
    )?;

    let cache_mode = cache_mode(matches);
    let task_cache = cache_mode
        .writes_or_reads()
        .then(|| TaskCache::new(workspace.root.join(".toven/cache").join(CACHE_DIRECTORY)))
        .transpose()?;
    let decisions = prepare_cache_decisions(&cache_plan, &cache_mode, task_cache.as_ref())?;
    let external_cancellation = cancellation.is_some();
    let cancellation = cancellation.unwrap_or_else(SharedCancellation::new);
    let ctrl_c_handler = (persistent_lifecycle == PersistentLifecycle::Block
        && !external_cancellation)
        .then(|| spawn_ctrl_c_handler(cancellation.clone()))
        .transpose()?;
    let options = RunOptions {
        timeout: matches
            .get_one::<u64>("timeout-seconds")
            .map(|seconds| Duration::from_secs(*seconds)),
        cancel_on_ctrl_c: false,
        cancellation: Some(cancellation),
    };
    let mut reporter = RunReporter::new(output_format, stdout, exec_plan.units.len())?;
    reporter.plan_prepared(&workspace.name, &workspace.root.display().to_string())?;
    for unit in &exec_plan.units {
        reporter.plan_unit(unit, &workspace.root)?;
    }
    for decision in decisions.values() {
        reporter.cache_decision(decision)?;
    }

    let has_finite_units = exec_plan.units.iter().any(|unit| !unit.persistent);
    let mut persistent_processes = Vec::new();
    for unit in exec_plan.units {
        match execute_unit(
            unit,
            &workspace.root,
            &decisions,
            task_cache.as_ref(),
            &options,
            &mut reporter,
            stderr,
        ) {
            Ok(Some(process)) => persistent_processes.push(process),
            Ok(None) => {}
            Err(error) => {
                let _ = reporter.run_failed(&error);
                let _ = stop_ctrl_c_handler(ctrl_c_handler);
                let _ = shutdown_active_processes(persistent_processes);
                return Err(error);
            }
        }
    }
    if persistent_lifecycle == PersistentLifecycle::KeepAlive {
        reporter.run_succeeded()?;
        return Ok(persistent_processes);
    }

    while let Some(active) = persistent_processes.pop() {
        let result = if has_finite_units {
            active.process.shutdown()
        } else {
            active.process.wait()
        };
        if let Err(error) = result {
            let _ = reporter.run_failed(&error);
            let _ = stop_ctrl_c_handler(ctrl_c_handler);
            let _ = shutdown_active_processes(persistent_processes);
            return Err(error);
        }
    }
    stop_ctrl_c_handler(ctrl_c_handler)?;
    reporter.run_succeeded()?;
    Ok(Vec::new())
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum PersistentLifecycle {
    Block,
    KeepAlive,
}

pub(super) struct ActivePersistentProcess {
    modules: BTreeSet<ModuleId>,
    process: PersistentProcess,
}

impl ActivePersistentProcess {
    pub(super) fn is_affected_by(&self, modules: &BTreeSet<ModuleId>) -> bool {
        self.modules.is_empty() || !self.modules.is_disjoint(modules)
    }

    pub(super) fn shutdown(self) -> AppResult<()> {
        self.process.shutdown()
    }
}

fn select_plans(
    matches: &ArgMatches,
    workspace: &Workspace,
    discovered: &[DiscoveredTaskProfile],
    passthrough_args: &[String],
    module_filter: Option<&BTreeSet<ModuleId>>,
) -> AppResult<(Plan, Plan)> {
    if let Some(module_filter) = module_filter {
        let exec_plan = plan_discovered_task_profiles(
            workspace.clone(),
            discovered,
            passthrough_args,
            Some(module_filter),
        )?;
        let modules = modules_from_discovered(discovered)?;
        let cache_filter = dependency_closure(&modules, module_filter)?;
        let cache_plan = plan_discovered_task_profiles(
            workspace.clone(),
            discovered,
            passthrough_args,
            Some(&cache_filter),
        )?;
        return Ok((exec_plan, cache_plan));
    }

    if matches.get_flag("affected") {
        let changes = resolve_affected_changes(workspace, matches)?;
        let modules = modules_from_discovered(discovered)?;
        let affected = resolve_affected_modules(changes, &modules)?;
        let exec_plan = plan_discovered_task_profiles(
            workspace.clone(),
            discovered,
            passthrough_args,
            Some(&affected.closure),
        )?;
        let cache_filter = dependency_closure(&modules, &affected.closure)?;
        let cache_plan = plan_discovered_task_profiles(
            workspace.clone(),
            discovered,
            passthrough_args,
            Some(&cache_filter),
        )?;
        return Ok((exec_plan, cache_plan));
    }

    reject_unused_affected_flags(matches)?;
    let full_plan =
        plan_discovered_task_profiles(workspace.clone(), discovered, passthrough_args, None)?;
    Ok((full_plan.clone(), full_plan))
}

fn execute_unit<W, E>(
    unit: ExecutionUnit,
    workspace_root: &std::path::Path,
    decisions: &CacheDecisions,
    task_cache: Option<&TaskCache>,
    options: &RunOptions,
    reporter: &mut RunReporter<'_, W>,
    stderr: &mut E,
) -> AppResult<Option<ActivePersistentProcess>>
where
    W: Write,
    E: Write,
{
    let misses = unit
        .modules
        .iter()
        .filter(|module| {
            !decision_for(decisions, &unit, &module.name).is_some_and(CacheDecision::is_hit)
        })
        .cloned()
        .collect::<Vec<_>>();

    if misses.is_empty() {
        for module in &unit.modules {
            reporter.cache_hit(&unit, module, false)?;
        }
        reporter.unit_skipped(&unit)?;
        return Ok(None);
    }

    if unit.mode == ExecutionMode::WorkspaceOnce {
        for module in &unit.modules {
            if decision_for(decisions, &unit, &module.name).is_some_and(CacheDecision::is_hit) {
                reporter.cache_hit(&unit, module, true)?;
            }
        }
    }

    let executable_unit = if unit.mode == ExecutionMode::WorkspaceOnce {
        unit
    } else {
        let mut filtered = unit;
        filtered.modules = misses;
        filtered
    };

    reporter.unit_started(&executable_unit)?;
    let mut cache_argv = None;
    let persistent_process = if executable_unit.persistent {
        let (output, process) = start_persistent_execution_unit_with_output(
            &executable_unit,
            workspace_root,
            options,
            persistent_output(reporter.format()),
        )?;
        reporter.persistent_ready(&executable_unit)?;
        if output.result.stdout_truncated {
            writeln!(stderr, "warning: stdout capture truncated").map_err(AppError::internal)?;
        }
        if output.result.stderr_truncated {
            writeln!(stderr, "warning: stderr capture truncated").map_err(AppError::internal)?;
        }
        Some(ActivePersistentProcess {
            modules: executable_unit
                .modules
                .iter()
                .map(|module| module.name.clone())
                .collect(),
            process,
        })
    } else {
        let output = run_execution_unit(&executable_unit, workspace_root, options)?;
        reporter.child_stdout(stderr, &output.result.stdout_bytes)?;
        stderr
            .write_all(&output.result.stderr_bytes)
            .map_err(AppError::internal)?;
        if output.result.stdout_truncated {
            writeln!(stderr, "warning: stdout capture truncated").map_err(AppError::internal)?;
        }
        if output.result.stderr_truncated {
            writeln!(stderr, "warning: stderr capture truncated").map_err(AppError::internal)?;
        }
        reporter.unit_finished(&executable_unit, &output.result, output.cancelled)?;
        if !output.result.success() {
            return Err(process_error(
                &executable_unit,
                &output.result,
                output.cancelled,
            ));
        }
        cache_argv = Some(output.argv);
        None
    };

    if let (Some(task_cache), Some(argv)) = (task_cache, cache_argv.as_ref()) {
        for module in &executable_unit.modules {
            if let Some(decision) = decision_for(decisions, &executable_unit, &module.name) {
                task_cache.write_success(decision, argv)?;
            }
        }
    }
    Ok(persistent_process)
}

const fn persistent_output(format: OutputFormat) -> PersistentOutput {
    match format {
        OutputFormat::Human => PersistentOutput::forward(
            PersistentOutputStream::Stdout,
            PersistentOutputStream::Stderr,
        ),
        OutputFormat::Jsonl => PersistentOutput::forward(
            PersistentOutputStream::Stderr,
            PersistentOutputStream::Stderr,
        ),
    }
}

fn shutdown_active_processes(processes: Vec<ActivePersistentProcess>) -> AppResult<()> {
    let mut first_error = None;
    for process in processes {
        if let Err(error) = process.shutdown() {
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or_else(|| Ok(()), Err)
}

fn decision_for<'a>(
    decisions: &'a CacheDecisions,
    unit: &ExecutionUnit,
    module: &ModuleId,
) -> Option<&'a CacheDecision> {
    decisions.get(&(unit.profile.clone(), module.clone()))
}

fn process_error(
    unit: &ExecutionUnit,
    result: &rskit_process::ProcessResult,
    cancelled: bool,
) -> AppError {
    if result.timed_out {
        return AppError::new(
            ErrorCode::Timeout,
            format!("execution unit '{}' timed out", unit.id),
        );
    }
    if cancelled {
        return AppError::new(
            ErrorCode::Cancelled,
            format!("execution unit '{}' was cancelled", unit.id),
        );
    }
    AppError::new(
        ErrorCode::Internal,
        format!(
            "execution unit '{}' failed with exit code {}",
            unit.id,
            result
                .exit_code
                .map_or_else(|| "signal".to_string(), |code| code.to_string())
        ),
    )
}

fn dependency_closure(
    modules: &[Module],
    roots: &BTreeSet<ModuleId>,
) -> AppResult<BTreeSet<ModuleId>> {
    let by_name = modules
        .iter()
        .map(|module| (module.name.clone(), module))
        .collect::<BTreeMap<_, _>>();
    let mut closure = roots.clone();
    let mut stack = roots.iter().cloned().collect::<Vec<_>>();
    while let Some(module_id) = stack.pop() {
        let Some(module) = by_name.get(&module_id) else {
            return Err(AppError::invalid_input(
                "modules",
                format!("module '{module_id}' is referenced but was not discovered"),
            ));
        };
        for dependency in &module.dependencies {
            if !closure.insert(dependency.clone()) {
                continue;
            }
            stack.push(dependency.clone());
        }
    }
    Ok(closure)
}

fn reject_unused_affected_flags(matches: &ArgMatches) -> AppResult<()> {
    if matches.contains_id("base") {
        return Err(AppError::invalid_input(
            "base",
            "--base can only be used with --affected",
        ));
    }
    if matches.get_flag("merge-base") {
        return Err(AppError::invalid_input(
            "merge-base",
            "--merge-base can only be used with --affected",
        ));
    }
    Ok(())
}

fn cache_mode(matches: &ArgMatches) -> CacheMode {
    if matches.get_flag("no-cache") {
        return CacheMode::Disabled {
            reason: "--no-cache was supplied".to_string(),
        };
    }
    if matches.get_flag("force") {
        return CacheMode::Force;
    }
    CacheMode::ReadWrite
}

trait CacheModeExt {
    fn writes_or_reads(&self) -> bool;
}

impl CacheModeExt for CacheMode {
    fn writes_or_reads(&self) -> bool {
        matches!(self, Self::ReadWrite | Self::Force)
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, time::Duration};

    use std::collections::BTreeMap;

    use super::{dependency_closure, execute_unit, process_error};
    use crate::cache::decision::{CacheDecision, CacheState};
    use crate::cache::key::CacheKey;
    use crate::core::{CommandOrigin, ExecutionMode, ExecutionUnit, Module};
    use crate::report::{OutputFormat, RunReporter};

    #[test]
    fn process_error_reports_timeout() {
        let error = process_error(&unit(), &result(None, true), false);

        assert_eq!(error.code, crate::core::ErrorCode::Timeout);
        assert!(error.message.contains("timed out"));
    }

    #[test]
    fn process_error_reports_cancellation() {
        let error = process_error(&unit(), &result(None, false), true);

        assert_eq!(error.code, crate::core::ErrorCode::Cancelled);
        assert!(error.message.contains("cancelled"));
    }

    #[test]
    fn process_error_reports_exit_code() {
        let error = process_error(&unit(), &result(Some(2), false), false);

        assert_eq!(error.code, crate::core::ErrorCode::Internal);
        assert!(error.message.contains("exit code 2"));
    }

    #[test]
    fn dependency_closure_includes_transitive_dependencies() {
        let modules = vec![
            module("app", &["service"]),
            module("service", &["core"]),
            module("core", &[]),
            module("unrelated", &[]),
        ];
        let roots =
            std::iter::once(crate::core::ModuleId::new("app").expect("module id")).collect();

        let closure = dependency_closure(&modules, &roots).expect("dependency closure computes");

        assert!(closure.contains(&crate::core::ModuleId::new("app").expect("module id")));
        assert!(closure.contains(&crate::core::ModuleId::new("service").expect("module id")));
        assert!(closure.contains(&crate::core::ModuleId::new("core").expect("module id")));
        assert!(!closure.contains(&crate::core::ModuleId::new("unrelated").expect("module id")));
    }

    #[test]
    fn workspace_once_reports_hit_modules_that_are_rerun_with_misses() {
        let root = rskit_testutil::test_workspace!("run-workspace-once-hit-message");
        let unit = ExecutionUnit {
            id: "workspace-test".to_string(),
            profile: "profile".to_string(),
            scope: None,
            task: "test".to_string(),
            command_origin: CommandOrigin::DirectArgv,
            mode: ExecutionMode::WorkspaceOnce,
            resource_group: String::new(),
            modules: vec![module("hit", &[]), module("miss", &[])],
            argv_template: vec![
                "sh".to_string(),
                "-c".to_string(),
                "printf workspace-once".to_string(),
            ],
            module_arg_template: Vec::new(),
            passthrough_args: Vec::new(),
            cache_args: false,
            persistent: false,
            readiness: crate::core::PersistentReadiness::Started,
            readiness_timeout: Duration::from_secs(30),
            shared_inputs: Vec::new(),
        };
        let mut decisions = BTreeMap::new();
        decisions.insert(
            (
                "profile".to_string(),
                crate::core::ModuleId::new("hit").expect("module id"),
            ),
            decision("hit", CacheState::Hit),
        );
        decisions.insert(
            (
                "profile".to_string(),
                crate::core::ModuleId::new("miss").expect("module id"),
            ),
            decision(
                "miss",
                CacheState::Miss {
                    reason: "no cache record".to_string(),
                },
            ),
        );
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut reporter =
            RunReporter::new(OutputFormat::Human, &mut stdout, 1).expect("reporter initializes");

        execute_unit(
            unit,
            root.path(),
            &decisions,
            None,
            &crate::exec::RunOptions {
                timeout: None,
                cancel_on_ctrl_c: false,
                cancellation: None,
            },
            &mut reporter,
            &mut stderr,
        )
        .expect("workspace-once unit executes");
        drop(reporter);

        assert!(stderr.is_empty());
        let stdout = String::from_utf8(stdout).expect("stdout is utf-8");
        assert!(stdout.contains("cache hit (re-run as part of workspace-once): hit test"));
        assert!(stdout.contains("workspace-once"));
    }

    #[test]
    fn persistent_process_tracks_only_started_modules() {
        let root = rskit_testutil::test_workspace!("run-persistent-started-modules");
        let mut unit = unit();
        unit.mode = ExecutionMode::SpawnEach;
        unit.modules = vec![module("hit", &[]), module("miss", &[])];
        unit.argv_template = vec!["sleep".to_string(), "2".to_string()];
        unit.persistent = true;
        let mut decisions = BTreeMap::new();
        decisions.insert(
            (
                "profile".to_string(),
                crate::core::ModuleId::new("hit").expect("module id"),
            ),
            decision("hit", CacheState::Hit),
        );
        decisions.insert(
            (
                "profile".to_string(),
                crate::core::ModuleId::new("miss").expect("module id"),
            ),
            decision(
                "miss",
                CacheState::Miss {
                    reason: "no cache record".to_string(),
                },
            ),
        );
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut reporter =
            RunReporter::new(OutputFormat::Human, &mut stdout, 1).expect("reporter initializes");

        let active = execute_unit(
            unit,
            root.path(),
            &decisions,
            None,
            &crate::exec::RunOptions {
                timeout: None,
                cancel_on_ctrl_c: false,
                cancellation: None,
            },
            &mut reporter,
            &mut stderr,
        )
        .expect("persistent unit starts")
        .expect("persistent process is active");

        let hit = std::iter::once(crate::core::ModuleId::new("hit").expect("module id")).collect();
        let miss =
            std::iter::once(crate::core::ModuleId::new("miss").expect("module id")).collect();
        assert!(!active.is_affected_by(&hit));
        assert!(active.is_affected_by(&miss));
        active.shutdown().expect("persistent process shuts down");
    }

    #[test]
    fn persistent_ready_does_not_report_unit_finished() {
        let root = rskit_testutil::test_workspace!("run-persistent-ready-jsonl");
        let mut unit = unit();
        unit.mode = ExecutionMode::WorkspaceOnce;
        unit.modules = vec![module("miss", &[])];
        unit.argv_template = vec!["sleep".to_string(), "2".to_string()];
        unit.persistent = true;
        let decisions = BTreeMap::new();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut reporter =
            RunReporter::new(OutputFormat::Jsonl, &mut stdout, 1).expect("reporter initializes");

        let active = execute_unit(
            unit,
            root.path(),
            &decisions,
            None,
            &crate::exec::RunOptions {
                timeout: None,
                cancel_on_ctrl_c: false,
                cancellation: None,
            },
            &mut reporter,
            &mut stderr,
        )
        .expect("persistent unit starts")
        .expect("persistent process is active");
        active.shutdown().expect("persistent process shuts down");
        drop(reporter);

        let stdout = String::from_utf8(stdout).expect("stdout is utf-8");
        let events = stdout
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("valid jsonl"))
            .map(|event| event["event"].as_str().expect("event string").to_string())
            .collect::<Vec<_>>();
        assert!(events.iter().any(|event| event == "persistent.ready"));
        assert!(!events.iter().any(|event| event == "unit.finished"));
    }

    fn unit() -> ExecutionUnit {
        ExecutionUnit {
            id: "unit".to_string(),
            profile: "profile".to_string(),
            scope: None,
            task: "test".to_string(),
            command_origin: CommandOrigin::DirectArgv,
            mode: ExecutionMode::SpawnEach,
            resource_group: String::new(),
            modules: Vec::new(),
            argv_template: Vec::new(),
            module_arg_template: Vec::new(),
            passthrough_args: Vec::new(),
            cache_args: false,
            persistent: false,
            readiness: crate::core::PersistentReadiness::Started,
            readiness_timeout: Duration::from_secs(30),
            shared_inputs: Vec::new(),
        }
    }

    fn result(exit_code: Option<i32>, timed_out: bool) -> rskit_process::ProcessResult {
        rskit_process::ProcessResult::completed(
            exit_code,
            Vec::new(),
            Vec::new(),
            false,
            false,
            Duration::ZERO,
            timed_out,
            false,
        )
    }

    fn module(name: &str, dependencies: &[&str]) -> Module {
        Module {
            name: crate::core::ModuleId::new(name).expect("module id"),
            package: None,
            root: name.into(),
            manifest: Some(PathBuf::from("Cargo.toml")),
            dependencies: dependencies
                .iter()
                .map(|dependency| crate::core::ModuleId::new(*dependency).expect("module id"))
                .collect(),
            source_patterns: Vec::new(),
        }
    }

    fn decision(module: &str, state: CacheState) -> CacheDecision {
        CacheDecision {
            profile: "profile".to_string(),
            module: crate::core::ModuleId::new(module).expect("module id"),
            task: "test".to_string(),
            key: CacheKey::new(format!("key-{module}")),
            source_hash: "source".to_string(),
            dep_hash: "dep".to_string(),
            task_hash: "task".to_string(),
            shared_hash: "shared".to_string(),
            state,
        }
    }
}
