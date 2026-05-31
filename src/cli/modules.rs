//! `toven modules` command.

use std::{io::Write, path::PathBuf};

use clap::ArgMatches;

use crate::{
    adapter::AdapterRegistry,
    cli::affected::modules_from_discovered,
    config::load_workspace,
    core::{AppError, AppResult, Module},
    engine::discover_workspace_task_profiles,
};

pub(super) fn run_modules(matches: &ArgMatches, stdout: &mut impl Write) -> AppResult<()> {
    let config = PathBuf::from(
        matches
            .get_one::<String>("config")
            .expect("clap supplies the modules config default"),
    );
    let task = matches
        .get_one::<String>("task")
        .expect("clap supplies the modules task default")
        .as_str();
    let workspace = load_workspace(config)?;
    let discovered =
        discover_workspace_task_profiles(&workspace, task, &AdapterRegistry::default())?;
    let modules = modules_from_discovered(&discovered)?;

    if modules.is_empty() {
        writeln!(stdout, "modules: none").map_err(AppError::internal)?;
        return Ok(());
    }

    writeln!(stdout, "modules:").map_err(AppError::internal)?;
    for module in modules {
        render_module(stdout, &module)?;
    }
    Ok(())
}

fn render_module(stdout: &mut impl Write, module: &Module) -> AppResult<()> {
    writeln!(stdout, "- {}/{}", module.scope_id, module.name).map_err(AppError::internal)?;
    writeln!(stdout, "  adapter: {}", module.adapter_id).map_err(AppError::internal)?;
    writeln!(stdout, "  root: {}", module.root.display()).map_err(AppError::internal)?;
    if let Some(package) = &module.package {
        writeln!(stdout, "  package: {package}").map_err(AppError::internal)?;
    }
    if module.dependencies.is_empty() {
        writeln!(stdout, "  dependencies: none").map_err(AppError::internal)?;
    } else {
        let dependencies = module
            .dependencies
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(stdout, "  dependencies: {dependencies}").map_err(AppError::internal)?;
    }
    Ok(())
}
