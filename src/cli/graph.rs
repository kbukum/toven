//! `toven graph` command.

use std::{io::Write, path::PathBuf};

use clap::ArgMatches;

use crate::{
    adapter::AdapterRegistry,
    cli::affected::modules_from_discovered,
    config::load_workspace,
    core::{AppError, AppResult, Module, scoped_module_display, scoped_module_key},
    engine::{
        discover_workspace_task_profiles,
        graph::{dependency_key_for, selected_modules_by_name},
    },
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

    let modules_by_key = modules
        .iter()
        .map(|module| (scoped_module_key(module), module))
        .collect::<std::collections::BTreeMap<_, _>>();
    let selected_by_name = selected_modules_by_name(modules_by_key.keys());
    for module in modules {
        let dependencies = module
            .dependencies
            .iter()
            .filter_map(|dependency| dependency_key_for(module, dependency, &selected_by_name))
            .map(|dependency| scoped_module_display(&dependency))
            .collect::<Vec<_>>()
            .join(", ");
        if dependencies.is_empty() {
            writeln!(stdout, "{}: none", module_id(module)).map_err(AppError::internal)?;
        } else {
            writeln!(stdout, "{}: {dependencies}", module_id(module))
                .map_err(AppError::internal)?;
        }
    }
    Ok(())
}

fn render_dot(stdout: &mut impl Write, modules: &[Module]) -> AppResult<()> {
    writeln!(stdout, "digraph toven {{").map_err(AppError::internal)?;
    let modules_by_key = modules
        .iter()
        .map(|module| (scoped_module_key(module), module))
        .collect::<std::collections::BTreeMap<_, _>>();
    let selected_by_name = selected_modules_by_name(modules_by_key.keys());
    for module in modules {
        writeln!(stdout, "  \"{}\";", escape_dot(&module_id(module)))
            .map_err(AppError::internal)?;
        for dependency in &module.dependencies {
            let Some(dependency) = dependency_key_for(module, dependency, &selected_by_name) else {
                continue;
            };
            writeln!(
                stdout,
                "  \"{}\" -> \"{}\";",
                escape_dot(&module_id(module)),
                escape_dot(&scoped_module_display(&dependency))
            )
            .map_err(AppError::internal)?;
        }
    }
    writeln!(stdout, "}}").map_err(AppError::internal)
}

fn escape_dot(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn module_id(module: &Module) -> String {
    format!("{}/{}", module.scope_id, module.name)
}
