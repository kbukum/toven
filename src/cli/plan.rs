//! `toven plan` command.

use std::{io::Write, path::PathBuf};

use clap::ArgMatches;

use crate::{
    config::load_workspace, core::AppResult, engine::plan_workspace, lang::LangRegistry,
    report::render_human_plan,
};

pub(super) fn run_plan(matches: &ArgMatches, stdout: &mut impl Write) -> AppResult<()> {
    let config = matches
        .get_one::<String>("config")
        .map_or_else(|| PathBuf::from("toven.toml"), PathBuf::from);
    let task = matches
        .get_one::<String>("task")
        .map_or("test", String::as_str);
    let passthrough_args = matches
        .get_many::<String>("args")
        .map(|values| values.cloned().collect::<Vec<_>>())
        .unwrap_or_default();

    let workspace = load_workspace(config)?;
    let plan = plan_workspace(workspace, task, &passthrough_args, &LangRegistry::default())?;
    writeln!(stdout, "{}", render_human_plan(&plan)?).map_err(crate::core::AppError::internal)
}
