//! `toven tasks`: the runnable-task discovery verb.
//!
//! Answers "what can I run?" by rendering the engine's [`TaskCatalog`] — the
//! fully resolved task set per ecosystem, carrying the **canonical** name a user
//! types (`format`, never the underlying `fmt` subcommand), its origin, fan-out,
//! and persistence. A human table by default; a stable JSON-lines schema under
//! `--output jsonl` for scripts. An optional `[name]` narrows to one task's
//! detail (argv template + shared inputs). Renders data the engine returns —
//! this verb prints, the projection does not.

use rskit_cli::{ExitCode, OutputKV, OutputTable};
use rskit_errors::{AppError, AppResult};
use toven_engine::plan::{TaskCatalog, TaskSummary, task_catalog};
use toven_ports::Provider;

use crate::flags::OutputKind;
use crate::host::{Project, resolve_output};

/// `toven tasks [name] [--output human|jsonl]`.
///
/// # Errors
/// Propagates [`task_catalog`] (Configure) failures, or a not-found error when a
/// `[name]` filter matches no resolved task.
pub(crate) fn tasks(
    providers: &[&dyn Provider],
    project: &Project,
    name: Option<&str>,
    output: Option<OutputKind>,
) -> AppResult<ExitCode> {
    let catalog = task_catalog(&project.document, providers)?;
    let catalog = match name {
        Some(filter) => filtered(catalog, filter)?,
        None => catalog,
    };
    match resolve_output(output, &project.document) {
        OutputKind::Jsonl => render_jsonl(&catalog)?,
        OutputKind::Human => render_human(&catalog, name.is_some()),
    }
    Ok(ExitCode::Success)
}

/// Keep only the tasks whose canonical name equals `filter`, erroring when none
/// match so a mistyped name is reported instead of printing an empty catalog.
fn filtered(mut catalog: TaskCatalog, filter: &str) -> AppResult<TaskCatalog> {
    for eco in &mut catalog.ecosystems {
        eco.tasks.retain(|task| task.name == filter);
    }
    catalog.ecosystems.retain(|eco| !eco.tasks.is_empty());
    if catalog.ecosystems.is_empty() {
        return Err(AppError::not_found(
            filter,
            Some("no such task; run `toven tasks` to list every runnable task"),
        ));
    }
    Ok(catalog)
}

/// Render the catalog as titled per-ecosystem tables (summary) or key-value
/// blocks (single-task detail).
fn render_human(catalog: &TaskCatalog, detail: bool) {
    if catalog.ecosystems.is_empty() {
        println!("no runnable tasks (no ecosystem with a loaded provider is configured)");
        return;
    }
    for eco in &catalog.ecosystems {
        if detail {
            for task in &eco.tasks {
                let mut kv = OutputKV::new();
                kv.add("ecosystem", eco.ecosystem.clone())
                    .add("task", task.name.clone())
                    .add("kind", task.kind.clone())
                    .add("origin", task.origin.as_str().to_string())
                    .add("fan-out", task.fan_out.as_str().to_string())
                    .add("persistent", task.persistent.to_string())
                    .add("argv", format!("{:?}", task.argv))
                    .add("shared-inputs", task.shared_inputs.join(", "));
                println!("{kv}");
            }
        } else {
            let mut table = OutputTable::new(vec!["Task", "Origin", "Fan-out", "Persistent"])
                .with_title(format!("{} tasks", eco.ecosystem));
            for task in &eco.tasks {
                table.add_row(vec![
                    task.name.clone(),
                    task.origin.as_str().to_string(),
                    task.fan_out.as_str().to_string(),
                    yes_no(task.persistent).to_string(),
                ]);
            }
            println!("{table}");
        }
    }
}

/// Render the catalog as one JSON object per task line (a stable schema).
///
/// # Errors
/// Propagates a serialization failure (never expected for these plain fields).
fn render_jsonl(catalog: &TaskCatalog) -> AppResult<()> {
    for eco in &catalog.ecosystems {
        for task in &eco.tasks {
            let line = serde_json::to_string(&TaskRecord::project(&eco.ecosystem, task))
                .map_err(AppError::internal)?;
            println!("{line}");
        }
    }
    Ok(())
}

/// The stable JSON record for one task in the `jsonl` projection.
#[derive(serde::Serialize)]
struct TaskRecord<'a> {
    ecosystem: &'a str,
    task: &'a str,
    kind: &'a str,
    origin: &'a str,
    fan_out: &'a str,
    persistent: bool,
    argv: &'a [String],
    shared_inputs: &'a [String],
}

impl<'a> TaskRecord<'a> {
    fn project(ecosystem: &'a str, task: &'a TaskSummary) -> Self {
        Self {
            ecosystem,
            task: &task.name,
            kind: &task.kind,
            origin: task.origin.as_str(),
            fan_out: task.fan_out.as_str(),
            persistent: task.persistent,
            argv: &task.argv,
            shared_inputs: &task.shared_inputs,
        }
    }
}

const fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}
