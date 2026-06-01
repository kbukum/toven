//! `toven <task>` execution command.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

use clap::ArgMatches;

use crate::{
    adapter::AdapterRegistry,
    cache::decision::{
        CacheDecision, CacheDecisions, CacheMode, TaskCache, prepare_cache_decisions,
    },
    cache::path::resolve_task_cache_root,
    cli::affected::{modules_from_discovered, resolve_affected_changes, resolve_affected_modules},
    config::load_workspace,
    core::{
        AppError, AppResult, ErrorCode, ExecutionMode, ExecutionUnit, Module, ModuleId, Plan,
        ScopedModuleKey, Workspace, scoped_module_key,
    },
    engine::{
        DiscoveredTaskProfile, discover_workspace_task_profiles,
        graph::resolve_selected_dependency_graph, plan_discovered_task_profiles,
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
    module_filter: Option<&BTreeSet<ScopedModuleKey>>,
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
    module_filter: Option<&BTreeSet<ScopedModuleKey>>,
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
    module_filter: Option<&BTreeSet<ScopedModuleKey>>,
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
        .then(|| resolve_task_cache_root(&workspace).and_then(TaskCache::new))
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
        cancellation: Some(cancellation),
        stream_output: output_format == OutputFormat::Human,
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
    let execution = execute_plan_units(
        exec_plan.units,
        &workspace.root,
        &decisions,
        task_cache.as_ref(),
        &options,
        &mut reporter,
        stderr,
    );
    match execution {
        Ok(processes) => {
            persistent_processes = processes;
        }
        Err(error) => {
            let _ = reporter.run_failed(&error);
            let _ = stop_ctrl_c_handler(ctrl_c_handler);
            let _ = shutdown_active_processes(persistent_processes);
            return Err(error);
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

fn execute_plan_units<W, E>(
    units: Vec<ExecutionUnit>,
    workspace_root: &Path,
    decisions: &CacheDecisions,
    task_cache: Option<&TaskCache>,
    options: &RunOptions,
    reporter: &mut RunReporter<'_, W>,
    stderr: &mut E,
) -> AppResult<Vec<ActivePersistentProcess>>
where
    W: Write,
    E: Write,
{
    if units.iter().any(|unit| unit.persistent) {
        return execute_plan_units_sequential(
            units,
            workspace_root,
            decisions,
            task_cache,
            options,
            reporter,
            stderr,
        );
    }

    execute_plan_units_wave_parallel(
        units,
        workspace_root,
        decisions,
        task_cache,
        options,
        reporter,
        stderr,
    )
}

fn execute_plan_units_sequential<W, E>(
    units: Vec<ExecutionUnit>,
    workspace_root: &Path,
    decisions: &CacheDecisions,
    task_cache: Option<&TaskCache>,
    options: &RunOptions,
    reporter: &mut RunReporter<'_, W>,
    stderr: &mut E,
) -> AppResult<Vec<ActivePersistentProcess>>
where
    W: Write,
    E: Write,
{
    let mut persistent_processes = Vec::new();
    for unit in units {
        match execute_unit(
            unit,
            workspace_root,
            decisions,
            task_cache,
            options,
            reporter,
            stderr,
        ) {
            Ok(Some(process)) => persistent_processes.push(process),
            Ok(None) => {}
            Err(error) => {
                return match shutdown_active_processes(persistent_processes) {
                    Ok(()) => Err(error),
                    Err(shutdown_error) => Err(error.with_cause(shutdown_error)),
                };
            }
        }
    }
    Ok(persistent_processes)
}

fn execute_plan_units_wave_parallel<W, E>(
    units: Vec<ExecutionUnit>,
    workspace_root: &Path,
    decisions: &CacheDecisions,
    task_cache: Option<&TaskCache>,
    options: &RunOptions,
    reporter: &mut RunReporter<'_, W>,
    stderr: &mut E,
) -> AppResult<Vec<ActivePersistentProcess>>
where
    W: Write,
    E: Write,
{
    let mut pending = units.into_iter().peekable();
    while let Some(unit) = pending.next() {
        let Some(wave_index) = wave_index_from_unit_id(&unit.id) else {
            execute_unit(
                unit,
                workspace_root,
                decisions,
                task_cache,
                options,
                reporter,
                stderr,
            )?;
            continue;
        };

        let mut wave_units = vec![unit];
        while let Some(next) = pending.peek() {
            if wave_index_from_unit_id(&next.id) == Some(wave_index) {
                let next = pending.next().expect("peeked unit exists");
                wave_units.push(next);
            } else {
                break;
            }
        }

        execute_wave_parallel(
            wave_units,
            workspace_root,
            decisions,
            task_cache,
            options,
            reporter,
            stderr,
        )?;
    }

    Ok(Vec::new())
}

fn execute_wave_parallel<W, E>(
    wave_units: Vec<ExecutionUnit>,
    workspace_root: &Path,
    decisions: &CacheDecisions,
    task_cache: Option<&TaskCache>,
    options: &RunOptions,
    reporter: &mut RunReporter<'_, W>,
    stderr: &mut E,
) -> AppResult<()>
where
    W: Write,
    E: Write,
{
    let mut groups = BTreeMap::<String, Vec<PreparedExecution>>::new();
    for (order, unit) in wave_units.into_iter().enumerate() {
        let Some(prepared) = prepare_execution(order, unit, decisions, reporter)? else {
            continue;
        };
        let resource_group = crate::exec::render_resource_group(&prepared.unit, workspace_root)?;
        groups.entry(resource_group).or_default().push(prepared);
    }

    if groups.is_empty() {
        return Ok(());
    }

    let workspace_root = workspace_root.to_path_buf();
    let options = options.clone();
    let (tx, rx) = mpsc::channel();

    std::thread::scope(|scope| -> AppResult<()> {
        for prepared_units in groups.into_values() {
            let tx = tx.clone();
            let workspace_root = workspace_root.clone();
            let options = options.clone();
            scope.spawn(move || {
                for prepared in prepared_units {
                    if is_run_cancelled(options.cancellation.as_ref()) {
                        break;
                    }
                    let result = run_execution_unit(&prepared.unit, &workspace_root, &options);
                    let should_stop = result.is_err();
                    if tx
                        .send(PreparedExecutionResult {
                            order: prepared.order,
                            unit: prepared.unit,
                            result,
                        })
                        .is_err()
                    {
                        break;
                    }
                    if should_stop {
                        cancel_run(options.cancellation.as_ref());
                        break;
                    }
                }
            });
        }
        drop(tx);

        let mut results = BTreeMap::new();
        let mut next_order = 0usize;
        let mut first_cancelled_error = None;

        // Drain the channel until all workers have exited. Workers break early on
        // cancellation without sending results for skipped units, so we cannot rely
        // on a fixed expected count; the channel simply closes when all workers exit.
        while let Ok(result) = rx.recv() {
            results.insert(result.order, result);
            while let Some(result) = results.remove(&next_order) {
                if let Err(error) = finalize_execution_result(
                    result, decisions, task_cache, &options, reporter, stderr,
                ) {
                    cancel_run(options.cancellation.as_ref());
                    if error.code == ErrorCode::Cancelled {
                        first_cancelled_error.get_or_insert(error);
                    } else {
                        return Err(error);
                    }
                }
                next_order += 1;
            }
        }

        // After the channel closes, finalize any buffered out-of-order results whose
        // lower-order predecessors were skipped due to cancellation. These are iterated
        // in ascending order (BTreeMap) so the first failure is reported.
        for (_, result) in results {
            if let Err(error) =
                finalize_execution_result(result, decisions, task_cache, &options, reporter, stderr)
            {
                cancel_run(options.cancellation.as_ref());
                if error.code == ErrorCode::Cancelled {
                    first_cancelled_error.get_or_insert(error);
                } else {
                    return Err(error);
                }
            }
        }

        first_cancelled_error.map_or(Ok(()), Err)
    })?;

    Ok(())
}

fn cancel_run(cancellation: Option<&SharedCancellation>) {
    if let Some(cancellation) = cancellation {
        cancellation.cancel();
    }
}

fn is_run_cancelled(cancellation: Option<&SharedCancellation>) -> bool {
    cancellation.is_some_and(SharedCancellation::is_cancelled)
}

#[derive(Debug, Clone)]
struct PreparedExecution {
    order: usize,
    unit: ExecutionUnit,
}

struct PreparedExecutionResult {
    order: usize,
    unit: ExecutionUnit,
    result: AppResult<crate::exec::RunOutput>,
}

fn prepare_execution<W>(
    order: usize,
    unit: ExecutionUnit,
    decisions: &CacheDecisions,
    reporter: &mut RunReporter<'_, W>,
) -> AppResult<Option<PreparedExecution>>
where
    W: Write,
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

    Ok(Some(PreparedExecution {
        order,
        unit: executable_unit,
    }))
}

fn finalize_execution_result<W, E>(
    result: PreparedExecutionResult,
    decisions: &CacheDecisions,
    task_cache: Option<&TaskCache>,
    options: &RunOptions,
    reporter: &mut RunReporter<'_, W>,
    stderr: &mut E,
) -> AppResult<()>
where
    W: Write,
    E: Write,
{
    let output = result.result?;

    if !options.stream_output {
        reporter.child_stdout(stderr, &output.result.stdout_bytes)?;
        stderr
            .write_all(&output.result.stderr_bytes)
            .map_err(AppError::internal)?;
    }
    if output.result.stdout_truncated {
        writeln!(stderr, "warning: stdout capture truncated").map_err(AppError::internal)?;
    }
    if output.result.stderr_truncated {
        writeln!(stderr, "warning: stderr capture truncated").map_err(AppError::internal)?;
    }

    reporter.unit_finished(&result.unit, &output.result, output.cancelled)?;
    if output.cancelled || !output.result.success() {
        return Err(process_error(
            &result.unit,
            &output.result,
            output.cancelled,
        ));
    }

    if let (Some(task_cache), true) = (task_cache, !result.unit.persistent) {
        for module in &result.unit.modules {
            if let Some(decision) = decision_for(decisions, &result.unit, &module.name) {
                task_cache.write_success(decision, &output.argv)?;
            }
        }
    }

    Ok(())
}

fn wave_index_from_unit_id(id: &str) -> Option<usize> {
    let (_, suffix) = id.rsplit_once("/w")?;
    let digit_len = suffix
        .chars()
        .take_while(char::is_ascii_digit)
        .map(char::len_utf8)
        .sum::<usize>();
    if digit_len == 0 || suffix.as_bytes().get(digit_len) != Some(&b'/') {
        return None;
    }
    suffix[..digit_len].parse().ok()
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum PersistentLifecycle {
    Block,
    KeepAlive,
}

pub(super) struct ActivePersistentProcess {
    modules: BTreeSet<ScopedModuleKey>,
    process: PersistentProcess,
}

impl ActivePersistentProcess {
    pub(super) fn is_affected_by(&self, modules: &BTreeSet<ScopedModuleKey>) -> bool {
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
    module_filter: Option<&BTreeSet<ScopedModuleKey>>,
) -> AppResult<(Plan, Plan)> {
    if let Some(module_filter) = module_filter {
        let exec_plan = plan_discovered_task_profiles(
            workspace.clone(),
            discovered,
            passthrough_args,
            Some(module_filter),
        )?;
        let modules = modules_from_discovered(discovered)?;
        let cache_filter =
            dependency_closure(&modules, &workspace.dependency_overlays, module_filter)?;
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
        let affected = resolve_affected_modules(changes, &modules, &workspace.dependency_overlays)?;
        let exec_plan = plan_discovered_task_profiles(
            workspace.clone(),
            discovered,
            passthrough_args,
            Some(&affected.closure),
        )?;
        let cache_filter =
            dependency_closure(&modules, &workspace.dependency_overlays, &affected.closure)?;
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
                .map(scoped_module_key)
                .collect(),
            process,
        })
    } else {
        let output = run_execution_unit(&executable_unit, workspace_root, options)?;
        if !options.stream_output {
            reporter.child_stdout(stderr, &output.result.stdout_bytes)?;
            stderr
                .write_all(&output.result.stderr_bytes)
                .map_err(AppError::internal)?;
        }
        if output.result.stdout_truncated {
            writeln!(stderr, "warning: stdout capture truncated").map_err(AppError::internal)?;
        }
        if output.result.stderr_truncated {
            writeln!(stderr, "warning: stderr capture truncated").map_err(AppError::internal)?;
        }
        reporter.unit_finished(&executable_unit, &output.result, output.cancelled)?;
        if output.cancelled || !output.result.success() {
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
    decisions.get(&(unit.scope_id.to_string(), module.clone()))
}

fn process_error(
    unit: &ExecutionUnit,
    result: &rskit_process::ProcessResult,
    cancelled: bool,
) -> AppError {
    if cancelled || result.cancelled {
        return AppError::new(
            ErrorCode::Cancelled,
            format!("execution unit '{}' was cancelled", unit.id),
        );
    }
    if result.timed_out {
        return AppError::new(
            ErrorCode::Timeout,
            format!("execution unit '{}' timed out", unit.id),
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
    overlays: &[crate::core::DependencyOverlay],
    roots: &BTreeSet<ScopedModuleKey>,
) -> AppResult<BTreeSet<ScopedModuleKey>> {
    let module_keys = modules
        .iter()
        .map(scoped_module_key)
        .collect::<BTreeSet<_>>();
    let graph = resolve_selected_dependency_graph(modules, overlays)?;
    let mut closure = roots.clone();
    let mut stack = roots.iter().cloned().collect::<Vec<_>>();
    while let Some(module_key) = stack.pop() {
        if !module_keys.contains(&module_key) {
            return Err(AppError::invalid_input(
                "modules",
                format!(
                    "module '{}/{}' is referenced but was not discovered",
                    module_key.0, module_key.1
                ),
            ));
        }
        for dependency_key in graph.dependencies(&module_key) {
            if !closure.insert(dependency_key.clone()) {
                continue;
            }
            stack.push(dependency_key);
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
    use std::{path::PathBuf, process::Stdio, time::Duration};

    use std::collections::BTreeMap;

    use super::{dependency_closure, execute_plan_units_sequential, execute_unit, process_error};
    use crate::cache::decision::{CacheDecision, CacheState};
    use crate::cache::key::CacheKey;
    use crate::core::{CommandOrigin, ErrorCode, ExecutionMode, ExecutionUnit, Module};
    use crate::exec::SharedCancellation;
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

        assert_eq!(error.code, ErrorCode::Cancelled);
        assert!(error.message.contains("cancelled"));
    }

    #[test]
    fn process_error_reports_result_cancellation_even_with_success_exit() {
        let error = process_error(&unit(), &cancelled_result(Some(0)), false);

        assert_eq!(error.code, ErrorCode::Cancelled);
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
        let roots = std::iter::once((
            "profile".to_string(),
            crate::core::ModuleId::new("app").expect("module id"),
        ))
        .collect();

        let closure =
            dependency_closure(&modules, &[], &roots).expect("dependency closure computes");

        assert!(closure.contains(&(
            "profile".to_string(),
            crate::core::ModuleId::new("app").expect("module id")
        )));
        assert!(closure.contains(&(
            "profile".to_string(),
            crate::core::ModuleId::new("service").expect("module id")
        )));
        assert!(closure.contains(&(
            "profile".to_string(),
            crate::core::ModuleId::new("core").expect("module id")
        )));
        assert!(!closure.contains(&(
            "profile".to_string(),
            crate::core::ModuleId::new("unrelated").expect("module id")
        )));
    }

    #[test]
    fn workspace_once_reports_hit_modules_that_are_rerun_with_misses() {
        let root = rskit_testutil::test_workspace!("run-workspace-once-hit-message");
        let unit = ExecutionUnit {
            id: "workspace-test".to_string(),
            scope_id: crate::core::ScopeId::new("profile").expect("scope id"),
            adapter_id: crate::core::AdapterId::new("rust").expect("adapter id"),
            task: "test".to_string(),
            command_origin: CommandOrigin::DirectArgv,
            task_origin: crate::core::TaskOrigin::ProjectDefault,
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
                cancellation: None,
                stream_output: false,
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
                cancellation: None,
                stream_output: false,
            },
            &mut reporter,
            &mut stderr,
        )
        .expect("persistent unit starts")
        .expect("persistent process is active");

        let hit = std::iter::once((
            "profile".to_string(),
            crate::core::ModuleId::new("hit").expect("module id"),
        ))
        .collect();
        let miss = std::iter::once((
            "profile".to_string(),
            crate::core::ModuleId::new("miss").expect("module id"),
        ))
        .collect();
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
                cancellation: None,
                stream_output: false,
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

    #[test]
    fn sequential_error_shuts_down_started_persistent_processes() {
        let root = rskit_testutil::test_workspace!("run-persistent-error-cleanup");
        let pid_file = root.path().join("persistent.pid");

        let mut persistent = unit();
        persistent.id = "persistent".to_string();
        persistent.mode = ExecutionMode::WorkspaceOnce;
        persistent.modules = vec![module("service", &[])];
        persistent.argv_template = vec![
            "sh".to_string(),
            "-c".to_string(),
            "printf %s $$ > \"$1\"; while :; do sleep 1; done".to_string(),
            "persistent".to_string(),
            pid_file.display().to_string(),
        ];
        persistent.persistent = true;
        persistent.readiness = crate::core::PersistentReadiness::Command(vec![
            "sh".to_string(),
            "-c".to_string(),
            "for _ in 1 2 3 4 5 6 7 8 9 10; do test -s \"$1\" && exit 0; sleep 0.01; done; exit 1"
                .to_string(),
            "ready".to_string(),
            pid_file.display().to_string(),
        ]);

        let mut failing = unit();
        failing.id = "failing".to_string();
        failing.mode = ExecutionMode::WorkspaceOnce;
        failing.modules = vec![module("failing", &[])];
        failing.argv_template = vec!["sh".to_string(), "-c".to_string(), "exit 7".to_string()];

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut reporter =
            RunReporter::new(OutputFormat::Human, &mut stdout, 2).expect("reporter initializes");

        let result = execute_plan_units_sequential(
            vec![persistent, failing],
            root.path(),
            &BTreeMap::new(),
            None,
            &crate::exec::RunOptions {
                timeout: None,
                cancellation: None,
                stream_output: false,
            },
            &mut reporter,
            &mut stderr,
        );

        let Err(error) = result else {
            panic!("failing unit should fail sequential execution");
        };
        assert_eq!(error.code, ErrorCode::Internal);
        let pid = std::fs::read_to_string(pid_file).expect("persistent process wrote pid");
        let still_alive = std::process::Command::new("kill")
            .arg("-0")
            .arg(pid.trim())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if still_alive {
            let _ = std::process::Command::new("kill")
                .arg("-KILL")
                .arg(pid.trim())
                .status();
        }
        assert!(!still_alive, "persistent process should be shut down");
    }

    #[test]
    fn wave_index_parser_uses_final_wave_segment() {
        assert_eq!(super::wave_index_from_unit_id("rust/test/w0/api"), Some(0));
        assert_eq!(
            super::wave_index_from_unit_id("rust/watch/w12/api"),
            Some(12)
        );
        assert_eq!(
            super::wave_index_from_unit_id("rust/foo/warm/w3/batch/m0"),
            Some(3)
        );
        assert_eq!(super::wave_index_from_unit_id("rust/test/workspace"), None);
        assert_eq!(super::wave_index_from_unit_id("rust/test/w/api"), None);
        assert_eq!(super::wave_index_from_unit_id("rust/test/w12"), None);
    }

    #[test]
    fn cancelled_execution_fails_even_if_process_returns_result() {
        let root = rskit_testutil::test_workspace!("run-cancelled-result");
        let mut unit = unit();
        unit.modules = vec![module("miss", &[])];
        unit.argv_template = vec![
            "sh".to_string(),
            "-c".to_string(),
            "trap 'exit 0' TERM; while true; do sleep 1; done".to_string(),
        ];
        let cancellation = SharedCancellation::new();
        cancellation.cancel();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut reporter =
            RunReporter::new(OutputFormat::Human, &mut stdout, 1).expect("reporter initializes");

        let result = execute_unit(
            unit,
            root.path(),
            &BTreeMap::new(),
            None,
            &crate::exec::RunOptions {
                timeout: None,
                cancellation: Some(cancellation),
                stream_output: false,
            },
            &mut reporter,
            &mut stderr,
        );
        let Err(error) = result else {
            panic!("cancelled process should fail execution");
        };

        assert_eq!(error.code, ErrorCode::Cancelled);
    }

    // Regression: when a unit fails with an internal error (run_execution_unit returns Err)
    // the worker calls cancel_run and breaks.  Other workers observe cancellation and break
    // without sending their remaining units.  The coordinator's reorder buffer may hold the
    // error result at a higher order while lower-order results from cancelled groups are
    // never sent.  The channel closes with fewer results than units.
    //
    // Old behaviour: coordinator hit a closed channel, saw is_run_cancelled=true, and
    // returned Cancelled — masking the original Internal error.
    // New behaviour: coordinator drains the channel then finalises any buffered results,
    // surfacing the original error.
    #[test]
    fn wave_parallel_surfaces_original_error_when_unit_is_skipped_by_cancellation() {
        let root = rskit_testutil::test_workspace!("run-wave-parallel-buffered-error");
        let cancellation = SharedCancellation::new();

        // Group "a" (BTreeMap order: first): spawn a nonexistent program → run_execution_unit
        // returns Err → worker sends the Err result, calls cancel_run, and breaks.
        // This unit gets order=1 because it is passed second in the wave vector.
        let mut erroring = unit();
        erroring.id = "unit/w0/erroring".to_string();
        erroring.resource_group = "a".to_string();
        erroring.modules = vec![module("err-module", &[])];
        erroring.argv_template = vec!["toven-nonexistent-program-xyz-12345".to_string()];

        // Group "b" (BTreeMap order: second): slow unit with order=0.  After group "a"
        // calls cancel_run, this worker sees is_run_cancelled=true at the top of its loop
        // and breaks without sending — leaving erroring(order=1) buffered in the coordinator
        // with no order=0 result ever arriving.
        let mut skippable = unit();
        skippable.id = "unit/w0/skippable".to_string();
        skippable.resource_group = "b".to_string();
        skippable.modules = vec![module("skip-module", &[])];
        skippable.argv_template = vec!["sh".to_string(), "-c".to_string(), "sleep 60".to_string()];

        // Pass skippable first so it gets order=0 and erroring gets order=1.
        let mut decisions = BTreeMap::new();
        for name in ["err-module", "skip-module"] {
            decisions.insert(
                (
                    "profile".to_string(),
                    crate::core::ModuleId::new(name).expect("module id"),
                ),
                decision(
                    name,
                    CacheState::Miss {
                        reason: "no cache record".to_string(),
                    },
                ),
            );
        }

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut reporter =
            RunReporter::new(OutputFormat::Human, &mut stdout, 2).expect("reporter initializes");

        let result = super::execute_wave_parallel(
            vec![skippable, erroring],
            root.path(),
            &decisions,
            None,
            &crate::exec::RunOptions {
                timeout: None,
                cancellation: Some(cancellation),
                stream_output: false,
            },
            &mut reporter,
            &mut stderr,
        );

        let Err(error) = result else {
            panic!("wave with erroring unit must return an error");
        };

        assert_ne!(
            error.code,
            ErrorCode::Cancelled,
            "original unit failure must not be masked by a Cancelled error from the closed channel"
        );
        assert_eq!(error.code, ErrorCode::Internal);
    }

    fn unit() -> ExecutionUnit {
        ExecutionUnit {
            id: "unit".to_string(),
            scope_id: crate::core::ScopeId::new("profile").expect("scope id"),
            adapter_id: crate::core::AdapterId::new("rust").expect("adapter id"),
            task: "test".to_string(),
            command_origin: CommandOrigin::DirectArgv,
            task_origin: crate::core::TaskOrigin::ProjectDefault,
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
        process_result(exit_code, timed_out, false)
    }

    fn cancelled_result(exit_code: Option<i32>) -> rskit_process::ProcessResult {
        process_result(exit_code, false, true)
    }

    fn process_result(
        exit_code: Option<i32>,
        timed_out: bool,
        cancelled: bool,
    ) -> rskit_process::ProcessResult {
        rskit_process::ProcessResult::completed(
            exit_code,
            Vec::new(),
            Vec::new(),
            false,
            false,
            Duration::ZERO,
            timed_out,
            cancelled,
        )
    }

    fn module(name: &str, dependencies: &[&str]) -> Module {
        Module {
            scope_id: crate::core::ScopeId::new("profile").expect("scope id"),
            adapter_id: crate::core::AdapterId::new("rust").expect("adapter id"),
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
            scope_id: "profile".to_string(),
            adapter_id: "rust".to_string(),
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
