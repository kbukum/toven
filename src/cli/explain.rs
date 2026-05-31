//! `toven explain` command.

use std::{io::Write, path::PathBuf};

use clap::ArgMatches;

use crate::{
    adapter::AdapterRegistry,
    cache::decision::{CACHE_DIRECTORY, CacheMode, CacheState, TaskCache, prepare_cache_decisions},
    cli::affected::{
        CliAffectedModules, modules_from_discovered, resolve_affected_changes,
        resolve_affected_modules,
    },
    config::load_workspace,
    core::{AppError, AppResult, ModuleId},
    engine::{discover_workspace_task_profiles, plan_discovered_task_profiles},
};

pub(super) fn run_explain(matches: &ArgMatches, stdout: &mut impl Write) -> AppResult<()> {
    let config = PathBuf::from(
        matches
            .get_one::<String>("config")
            .expect("clap supplies the explain config default"),
    );
    let module = ModuleId::new(
        matches
            .get_one::<String>("module")
            .expect("clap requires explain module")
            .clone(),
    )?;
    let task = matches
        .get_one::<String>("task")
        .expect("clap requires explain task")
        .as_str();

    let workspace = load_workspace(config)?;
    let registry = AdapterRegistry::default();
    let discovered = discover_workspace_task_profiles(&workspace, task, &registry)?;
    let modules = modules_from_discovered(&discovered)?;
    let affected =
        resolve_affected_modules(resolve_affected_changes(&workspace, matches)?, &modules)?;
    let full_plan = plan_discovered_task_profiles(workspace.clone(), &discovered, &[], None)?;
    let cache_mode = cache_mode(matches);
    let task_cache = cache_mode
        .writes_or_reads()
        .then(|| TaskCache::new(workspace.root.join(".toven/cache").join(CACHE_DIRECTORY)))
        .transpose()?;
    let decisions = prepare_cache_decisions(&full_plan, &cache_mode, task_cache.as_ref())?;

    let mut found = false;
    for ((profile, decision_module), decision) in &decisions {
        if decision_module != &module {
            continue;
        }
        found = true;
        writeln!(stdout, "module: {module}").map_err(AppError::internal)?;
        writeln!(stdout, "profile: {profile}").map_err(AppError::internal)?;
        writeln!(stdout, "task: {task}").map_err(AppError::internal)?;
        writeln!(stdout, "affected: {}", affected_reason(&affected, &module))
            .map_err(AppError::internal)?;
        if !affected.changed_paths.is_empty() {
            writeln!(stdout, "changed_paths:").map_err(AppError::internal)?;
            for path in &affected.changed_paths {
                writeln!(stdout, "- {}", path.display()).map_err(AppError::internal)?;
            }
        }
        if !affected.global_paths.is_empty() {
            writeln!(stdout, "global_paths:").map_err(AppError::internal)?;
            for path in &affected.global_paths {
                writeln!(stdout, "- {}", path.display()).map_err(AppError::internal)?;
            }
        }
        writeln!(stdout, "cache: {}", cache_state(&decision.state)).map_err(AppError::internal)?;
        writeln!(stdout, "key: {}", decision.key).map_err(AppError::internal)?;
        writeln!(stdout, "source_hash: {}", decision.source_hash).map_err(AppError::internal)?;
        writeln!(stdout, "dep_hash: {}", decision.dep_hash).map_err(AppError::internal)?;
        writeln!(stdout, "task_hash: {}", decision.task_hash).map_err(AppError::internal)?;
    }

    if !found {
        return Err(AppError::invalid_input(
            "module",
            format!("module '{module}' is not part of task '{task}'"),
        ));
    }
    Ok(())
}

fn affected_reason(affected: &CliAffectedModules, module: &ModuleId) -> &'static str {
    if affected.closure.is_empty() {
        "no"
    } else if !affected.global_paths.is_empty() {
        "yes (global)"
    } else if affected.direct.contains(module) {
        "yes (direct)"
    } else if affected.closure.contains(module) {
        "yes (dependent)"
    } else {
        "no"
    }
}

fn cache_state(state: &CacheState) -> String {
    match state {
        CacheState::Hit => "hit".to_string(),
        CacheState::Miss { reason } => format!("miss ({reason})"),
        CacheState::Disabled { reason } => format!("disabled ({reason})"),
        CacheState::Forced => "forced".to_string(),
    }
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
