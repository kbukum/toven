//! `toven <task>` execution command.

use std::{io::Write, path::PathBuf, thread, time::Duration};

use clap::ArgMatches;
use tokio_util::sync::CancellationToken;

use crate::{
    cache::decision::{
        CacheDecision, CacheDecisions, CacheMode, TaskCache, prepare_cache_decisions,
    },
    cli::affected::{modules_from_discovered, resolve_affected_changes, resolve_affected_modules},
    config::load_workspace,
    core::{AppError, AppResult, ErrorCode, ExecutionMode, ExecutionUnit, ModuleId},
    engine::{discover_workspace_task_profiles, plan_discovered_task_profiles},
    exec::{RunOptions, run_execution_unit},
    lang::LangRegistry,
};

pub(super) fn run_task(
    matches: &ArgMatches,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> AppResult<()> {
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
    let registry = LangRegistry::default();
    let discovered = discover_workspace_task_profiles(&workspace, task, &registry)?;
    let full_plan =
        plan_discovered_task_profiles(workspace.clone(), &discovered, &passthrough_args, None)?;
    let exec_plan = if matches.get_flag("affected") {
        let changes = resolve_affected_changes(&workspace, matches)?;
        let modules = modules_from_discovered(&discovered)?;
        let affected = resolve_affected_modules(changes, &modules)?;
        plan_discovered_task_profiles(
            workspace.clone(),
            &discovered,
            &passthrough_args,
            Some(&affected.closure),
        )?
    } else {
        reject_unused_affected_flags(matches)?;
        full_plan.clone()
    };

    let cache_mode = cache_mode(matches, &passthrough_args);
    let task_cache = cache_mode
        .writes_or_reads()
        .then(|| TaskCache::new(workspace.root.join(".toven/cache/v1")))
        .transpose()?;
    let decisions = prepare_cache_decisions(&full_plan, &cache_mode, task_cache.as_ref())?;
    let options = RunOptions {
        timeout: matches
            .get_one::<u64>("timeout-seconds")
            .map(|seconds| Duration::from_secs(*seconds)),
        cancel: install_cancellation_handler()?,
    };

    for unit in exec_plan.units {
        execute_unit(
            unit,
            &workspace.root,
            &decisions,
            task_cache.as_ref(),
            &options,
            stdout,
            stderr,
        )?;
    }
    Ok(())
}

fn install_cancellation_handler() -> AppResult<CancellationToken> {
    let token = CancellationToken::new();
    let cancel = token.clone();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            AppError::new(ErrorCode::Internal, "failed to create signal runtime").with_cause(error)
        })?;
    thread::Builder::new()
        .name("toven-signal".to_string())
        .spawn(move || {
            runtime.block_on(async move {
                if tokio::signal::ctrl_c().await.is_ok() {
                    cancel.cancel();
                }
            });
        })
        .map_err(|error| {
            AppError::new(
                ErrorCode::Internal,
                "failed to install cancellation handler",
            )
            .with_cause(error)
        })?;
    Ok(token)
}

fn execute_unit(
    unit: ExecutionUnit,
    workspace_root: &std::path::Path,
    decisions: &CacheDecisions,
    task_cache: Option<&TaskCache>,
    options: &RunOptions,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> AppResult<()> {
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
            writeln!(stdout, "cache hit: {} {}", module.name, unit.task)
                .map_err(AppError::internal)?;
        }
        return Ok(());
    }

    let executable_unit = if unit.mode == ExecutionMode::WorkspaceOnce {
        unit
    } else {
        let mut filtered = unit;
        filtered.modules = misses;
        filtered
    };

    writeln!(stdout, "run: {}", executable_unit.id).map_err(AppError::internal)?;
    let output = run_execution_unit(&executable_unit, workspace_root, options)?;
    stdout
        .write_all(&output.result.stdout_bytes)
        .map_err(AppError::internal)?;
    stderr
        .write_all(&output.result.stderr_bytes)
        .map_err(AppError::internal)?;
    if output.result.stdout_truncated {
        writeln!(stderr, "warning: stdout capture truncated").map_err(AppError::internal)?;
    }
    if output.result.stderr_truncated {
        writeln!(stderr, "warning: stderr capture truncated").map_err(AppError::internal)?;
    }
    if !output.result.success() {
        return Err(process_error(&executable_unit, &output.result, options));
    }

    if let Some(task_cache) = task_cache {
        for module in &executable_unit.modules {
            if let Some(decision) = decision_for(decisions, &executable_unit, &module.name) {
                task_cache.write_success(decision, &output.argv)?;
            }
        }
    }
    Ok(())
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
    options: &RunOptions,
) -> AppError {
    if result.timed_out {
        return AppError::new(
            ErrorCode::Timeout,
            format!("execution unit '{}' timed out", unit.id),
        );
    }
    if options.cancel.is_cancelled() {
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

fn cache_mode(matches: &ArgMatches, passthrough_args: &[String]) -> CacheMode {
    if matches.get_flag("no-cache") {
        return CacheMode::Disabled {
            reason: "--no-cache was supplied".to_string(),
        };
    }
    if !passthrough_args.is_empty() {
        return CacheMode::Disabled {
            reason: "passthrough args disable cache".to_string(),
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
