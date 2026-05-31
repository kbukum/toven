//! `toven graph` command.

use std::{io::Write, path::PathBuf};

use clap::ArgMatches;

use crate::{
    adapter::AdapterRegistry,
    cli::affected::modules_from_discovered,
    config::load_workspace,
    core::{AppError, AppResult, DependencyOverlay, Module, scoped_module_display},
    engine::{
        discover_workspace_task_profiles,
        graph::{DependencyOrigin, resolve_dependency_graph},
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
        "text" => render_text(stdout, &modules, &workspace.dependency_overlays),
        "dot" => render_dot(stdout, &modules, &workspace.dependency_overlays),
        _ => Err(AppError::invalid_input(
            "format",
            format!("unsupported graph format '{format}'"),
        )),
    }
}

fn render_text(
    stdout: &mut impl Write,
    modules: &[Module],
    overlays: &[DependencyOverlay],
) -> AppResult<()> {
    if modules.is_empty() {
        writeln!(stdout, "graph: empty").map_err(AppError::internal)?;
        return Ok(());
    }

    let graph = resolve_dependency_graph(modules, overlays)?;
    for module in modules {
        let module_key = crate::core::scoped_module_key(module);
        let dependencies = graph
            .dependencies(&module_key)
            .into_iter()
            .map(|dependency| {
                let origin = match graph.origin(&module_key, &dependency) {
                    Some(DependencyOrigin::Overlay) => " overlay",
                    _ => "",
                };
                format!("{}{}", scoped_module_display(&dependency), origin)
            })
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

fn render_dot(
    stdout: &mut impl Write,
    modules: &[Module],
    overlays: &[DependencyOverlay],
) -> AppResult<()> {
    writeln!(stdout, "digraph toven {{").map_err(AppError::internal)?;
    let graph = resolve_dependency_graph(modules, overlays)?;
    for module in modules {
        writeln!(stdout, "  \"{}\";", escape_dot(&module_id(module)))
            .map_err(AppError::internal)?;
        let module_key = crate::core::scoped_module_key(module);
        for dependency in graph.dependencies(&module_key) {
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
