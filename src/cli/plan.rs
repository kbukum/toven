//! `toven plan` command.

use std::{io::Write, path::PathBuf};

use clap::ArgMatches;

use crate::{
    cli::affected::resolve_affected_modules,
    config::load_workspace,
    core::AppResult,
    engine::{plan_workspace, plan_workspace_filtered},
    lang::LangRegistry,
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
        let affected = resolve_affected_modules(&workspace, task, matches)?;
        plan_workspace_filtered(
            workspace,
            task,
            &passthrough_args,
            &LangRegistry::default(),
            Some(&affected.closure),
        )?
    } else {
        plan_workspace(workspace, task, &passthrough_args, &LangRegistry::default())?
    };
    write!(stdout, "{}", render_human_plan(&plan)?).map_err(crate::core::AppError::internal)
}
