//! Human-readable plan report.

use std::fmt::Write as _;

use crate::{
    core::{AppResult, ExecutionMode, Plan},
    exec::{render_execution_unit, render_resource_group},
};

/// Render a plan for terminal review.
pub fn render_human_plan(plan: &Plan) -> AppResult<String> {
    let mut output = String::new();
    writeln!(&mut output, "workspace: {}", plan.workspace.name).expect("write string");
    writeln!(&mut output, "root: {}", plan.workspace.root.display()).expect("write string");

    if plan.units.is_empty() {
        writeln!(&mut output, "\nno units").expect("write string");
        return Ok(output);
    }

    for unit in &plan.units {
        let argv = render_execution_unit(unit, &plan.workspace.root)?;
        let resource_group = render_resource_group(unit, &plan.workspace.root)?;
        writeln!(&mut output, "\nunit: {}", unit.id).expect("write string");
        writeln!(
            &mut output,
            "profile: {} task: {} mode: {}",
            unit.profile,
            unit.task,
            execution_mode_name(unit.mode)
        )
        .expect("write string");
        writeln!(&mut output, "resource_group: {resource_group}").expect("write string");
        writeln!(&mut output, "argv: {}", argv.join(" ")).expect("write string");
        let modules = unit
            .modules
            .iter()
            .map(|module| module.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(&mut output, "modules: {modules}").expect("write string");
    }

    Ok(output)
}

const fn execution_mode_name(mode: ExecutionMode) -> &'static str {
    match mode {
        ExecutionMode::SpawnEach => "spawn-each",
        ExecutionMode::BatchReady => "batch-ready",
        ExecutionMode::WorkspaceOnce => "workspace-once",
    }
}
