//! `toven plan` command.

use std::{io::Write, path::PathBuf};

use clap::ArgMatches;

use crate::{
    adapter::AdapterRegistry,
    cli::affected::{modules_from_discovered, resolve_affected_changes, resolve_affected_modules},
    config::load_workspace,
    core::{AppError, AppResult},
    engine::{discover_workspace_task_profiles, plan_discovered_task_profiles, plan_workspace},
    report::render_human_plan,
};

pub(super) fn run_plan(matches: &ArgMatches, stdout: &mut impl Write) -> AppResult<()> {
    let config = PathBuf::from(
        matches
            .get_one::<String>("config")
            .expect("clap supplies the plan config default"),
    );
    let task = matches
        .get_one::<String>("task")
        .expect("clap supplies the plan task default")
        .as_str();
    let passthrough_args = matches
        .get_many::<String>("args")
        .map(|values| values.cloned().collect::<Vec<_>>())
        .unwrap_or_default();

    let workspace = load_workspace(config)?;
    let plan = if matches.get_flag("affected") {
        let changes = resolve_affected_changes(&workspace, matches)?;
        let discovered =
            discover_workspace_task_profiles(&workspace, task, &AdapterRegistry::default())?;
        let modules = modules_from_discovered(&discovered)?;
        let affected = resolve_affected_modules(changes, &modules)?;
        plan_discovered_task_profiles(
            workspace,
            &discovered,
            &passthrough_args,
            Some(&affected.closure),
        )?
    } else {
        reject_unused_affected_flags(matches)?;
        plan_workspace(
            workspace,
            task,
            &passthrough_args,
            &AdapterRegistry::default(),
        )?
    };
    write!(stdout, "{}", render_human_plan(&plan)?).map_err(crate::core::AppError::internal)
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
