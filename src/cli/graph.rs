//! `toven graph` command.

use std::{io::Write, path::PathBuf};

use clap::ArgMatches;

use crate::{
    adapter::AdapterRegistry,
    cli::affected::modules_from_discovered,
    config::load_workspace,
    core::{AppError, AppResult, Module},
    engine::discover_workspace_task_profiles,
};

pub(super) fn run_graph(matches: &ArgMatches, stdout: &mut impl Write) -> AppResult<()> {
    let config = PathBuf::from(
        matches
            .get_one::<String>("config")
            .expect("clap supplies the graph config default"),
    );
    let task = matches
        .get_one::<String>("task")
        .expect("clap supplies the graph task default")
        .as_str();
    let format = matches
        .get_one::<String>("format")
        .expect("clap supplies the graph format default")
        .as_str();
    let workspace = load_workspace(config)?;
    let discovered =
        discover_workspace_task_profiles(&workspace, task, &AdapterRegistry::default())?;
    let modules = modules_from_discovered(&discovered)?;

    match format {
        "text" => render_text(stdout, &modules),
        "dot" => render_dot(stdout, &modules),
        _ => Err(AppError::invalid_input(
            "format",
            format!("unsupported graph format '{format}'"),
        )),
    }
}

fn render_text(stdout: &mut impl Write, modules: &[Module]) -> AppResult<()> {
    if modules.is_empty() {
        writeln!(stdout, "graph: empty").map_err(AppError::internal)?;
        return Ok(());
    }

    for module in modules {
        if module.dependencies.is_empty() {
            writeln!(stdout, "{}: none", module.name).map_err(AppError::internal)?;
        } else {
            let dependencies = module
                .dependencies
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(stdout, "{}: {dependencies}", module.name).map_err(AppError::internal)?;
        }
    }
    Ok(())
}

fn render_dot(stdout: &mut impl Write, modules: &[Module]) -> AppResult<()> {
    writeln!(stdout, "digraph toven {{").map_err(AppError::internal)?;
    for module in modules {
        writeln!(stdout, "  \"{}\";", escape_dot(module.name.as_str()))
            .map_err(AppError::internal)?;
        for dependency in &module.dependencies {
            writeln!(
                stdout,
                "  \"{}\" -> \"{}\";",
                escape_dot(module.name.as_str()),
                escape_dot(dependency.as_str())
            )
            .map_err(AppError::internal)?;
        }
    }
    writeln!(stdout, "}}").map_err(AppError::internal)
}

fn escape_dot(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
