//! Introspection verbs: `modules`/`list`, `graph`/`deps`, `affected`, and
//! `explain`.
//!
//! `affected` and `explain` are thin projections over one immutable [`Plan`].
//! The shared [`build_plan`] runs the PLAN spine once with caching disabled
//! (introspection never executes, so cache verdicts are noise) and a reporter
//! that keeps stdout reserved for the projection while still surfacing
//! warnings, then the verb filters the resulting units.
//!
//! `modules` and `graph` project the validated discovered [`Graph`] directly,
//! so they do not depend on any particular task kind being configured or
//! schedulable.

use std::collections::BTreeSet;

use rskit_cli::{ExitCode, OutputKV, OutputTable};
use rskit_errors::{AppError, AppResult};
use toven_core::federation::resolve::PathDriverLocator;
use toven_core::plan::{
    CacheMode, FocusedPlan, PlanHost, PlanRequest, Selection, dependency_graph, plan_focused,
};
use toven_core::vcs::BaselineFlags;
use toven_engine::cache::NullCache;
use toven_engine::source::FsSourceDigest;
use toven_engine::toolchain::ProcessToolchainProber;
use toven_exec::ProcessToolRunner;
use toven_model::{Event, ExecutionUnit, Graph, ModuleKey, Plan};
use toven_ports::{Provider, Reporter, TaskIntent};

use crate::commands::selection::TaskSelection;
use crate::flags::{GraphFormat, OutputKind};
use crate::host::{Project, new_run_id, resolve_output};

/// A quiet [`Reporter`]: introspection prints its projection on stdout, while
/// warnings still go to stderr so warn-and-skip diagnostics are visible.
struct QuietReporter;

impl Reporter for QuietReporter {
    fn emit(&mut self, event: &Event) -> AppResult<()> {
        match event {
            Event::Warning { message } => eprintln!("warning: {message}"),
            // An introspection projection is machine-readable stdout, so the full-activation reason
            // rides the same stream as the projection it explains (the `affected` table), never
            // mixed onto stderr.
            Event::FullActivation { paths } => {
                println!(
                    "full activation: {} (affects all modules)",
                    paths.join(", ")
                );
            }
            _ => {}
        }
        Ok(())
    }
}

/// Build the single immutable [`Plan`] every introspection verb projects.
///
/// Caching is disabled (no execution happens, so a real cache verdict is
/// irrelevant and the cache port is never consulted) and the reporter is quiet
/// except for warnings.
///
/// # Errors
/// Propagates PLAN-spine failures (configuration, discovery, graph,
/// scheduling).
fn build_plan(
    providers: &[&dyn Provider],
    project: &Project,
    intent: TaskIntent,
    baseline: &BaselineFlags,
    selection: Selection,
) -> AppResult<Plan> {
    build_focused_plan(providers, project, intent, baseline, selection, None)
        .map(|focused| focused.plan)
}

/// Build the plan for the scope `selection` and, when `focus` is given, resolve
/// it to the module keys `explain` narrows its shown units to.
///
/// Caching is disabled and the reporter is quiet except for warnings, matching
/// [`build_plan`]. The `focus` selection never changes what is planned — the
/// plan is always built over `selection`, so a focused unit is the real batched
/// unit.
///
/// # Errors
/// Propagates PLAN-spine failures and any focus selector-resolution failure.
fn build_focused_plan(
    providers: &[&dyn Provider],
    project: &Project,
    intent: TaskIntent,
    baseline: &BaselineFlags,
    selection: Selection,
    focus: Option<&Selection>,
) -> AppResult<FocusedPlan> {
    let request = PlanRequest::new(
        new_run_id()?,
        project.document.project.name.clone(),
        intent,
        project.project_root.clone(),
    )
    .with_cache_mode(CacheMode::Disabled)
    .with_selection(selection);

    let opened = project.open_member_vcs(providers, baseline)?;
    let readers = opened.readers();
    let digest = FsSourceDigest::new(&project.project_root);
    let prober = ProcessToolchainProber::new(std::sync::Arc::new(ProcessToolRunner::new()));
    let cache = NullCache;
    let host = PlanHost::new(&readers, &digest, &prober, &cache);

    let mut reporter = QuietReporter;
    plan_focused(
        &request,
        focus,
        &project.document,
        providers,
        host,
        &mut reporter,
    )
}

/// Build the validated discovered module graph for topology introspection
/// verbs.
///
/// # Errors
/// Propagates Configure/Discover/Graph failures.
fn build_graph(providers: &[&dyn Provider], project: &Project) -> AppResult<Graph> {
    let locator = PathDriverLocator::new();
    let mut reporter = QuietReporter;
    dependency_graph(
        &project.project_root,
        &project.document,
        providers,
        &locator,
        &mut reporter,
    )
}

/// `toven modules` / `list` / `ls`: the discovered module set with its
/// workspace.
///
/// Renders a human table (Module, Workspace) by default, or a stable JSON-lines
/// projection under `--output jsonl` so tooling can consume the module set.
/// Both projections land on stdout per the introspection stream convention.
///
/// # Errors
/// Propagates [`build_graph`] failures, or a serialization failure in the jsonl
/// projection (never expected for these plain fields).
pub(crate) fn modules(
    providers: &[&dyn Provider],
    project: &Project,
    output: Option<OutputKind>,
) -> AppResult<ExitCode> {
    let graph = build_graph(providers, project)?;
    let rows = module_rows(&graph);
    match resolve_output(output, &project.document) {
        OutputKind::Jsonl => render_modules_jsonl(&rows)?,
        OutputKind::Human => render_modules_human(&rows),
    }
    Ok(ExitCode::Success)
}

/// `toven affected <task>`: the modules with a scheduled unit for `task`.
///
/// # Errors
/// Propagates [`build_plan`] failures.
pub(crate) fn affected(
    providers: &[&dyn Provider],
    project: &Project,
    intent: TaskIntent,
    selection: &TaskSelection,
) -> AppResult<ExitCode> {
    let resolved = selection.resolve(project.document.project.base_ref.as_deref())?;
    let plan = build_plan(providers, project, intent, &selection.baseline, resolved)?;
    print_module_table("Affected", plan_module_names(&plan));
    Ok(ExitCode::Success)
}

/// `toven graph` / `deps`: the discovered dependency edges (`--format
/// text|dot`).
///
/// # Errors
/// Propagates [`build_graph`] failures.
pub(crate) fn graph(
    providers: &[&dyn Provider],
    project: &Project,
    format: GraphFormat,
) -> AppResult<ExitCode> {
    let graph = build_graph(providers, project)?;
    let rendered = match format {
        GraphFormat::Text => render_graph_text(&graph),
        GraphFormat::Dot => toven_model::graph::render(&graph),
    };
    print!("{rendered}");
    Ok(ExitCode::Success)
}

/// `toven explain <task>`: the planned units for `task`, optionally focused to
/// a `--module`/`--workspace` selection.
///
/// An explicit selection focuses the projection: the plan is built over the
/// full active set (so each shown unit is the *real* batched unit) and only the
/// units whose members include a focus target are rendered, with the target
/// members marked and their co-batched siblings shown in full. Without an
/// explicit selection every planned unit is shown.
///
/// # Errors
/// Returns a not-found error when the plan schedules no unit for the task, or a
/// distinct not-found error when a focus target participates in no scheduled
/// unit; else propagates [`build_focused_plan`] failures.
pub(crate) fn explain(
    providers: &[&dyn Provider],
    project: &Project,
    intent: TaskIntent,
    selection: &TaskSelection,
) -> AppResult<ExitCode> {
    let (scope, focus) = selection.resolve_explain(project.document.project.base_ref.as_deref())?;
    let task_name = intent.name().to_string();
    let FocusedPlan { plan, focus } = build_focused_plan(
        providers,
        project,
        intent,
        &selection.baseline,
        scope,
        focus.as_ref(),
    )?;

    if plan.units.is_empty() {
        return Err(AppError::not_found(
            &task_name,
            Some("no planned unit for that task and selection"),
        ));
    }

    let shown: Vec<&ExecutionUnit> = match &focus {
        Some(targets) => plan
            .units
            .iter()
            .filter(|unit| unit.members.iter().any(|member| targets.contains(member)))
            .collect(),
        None => plan.units.iter().collect(),
    };

    if shown.is_empty() {
        return Err(AppError::not_found(
            &task_name,
            Some("the selected module is not in any planned unit for that task"),
        ));
    }

    for unit in shown {
        let mut detail = OutputKV::new();
        detail
            .add("unit", unit.id.clone())
            .add("representative", unit.module.to_string())
            .add(
                "modules",
                unit.members
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        if let Some(targets) = &focus {
            detail.add("target", focused_members(unit, targets));
        }
        detail
            .add("task", unit.task.clone())
            .add("origin", unit.origin.as_str().to_string())
            .add("argv", format!("{:?}", unit.argv))
            .add("persistent", unit.persistent.to_string())
            .add("depends_on", unit.depends_on.join(", "));
        println!("{detail}");
    }
    Ok(ExitCode::Success)
}

/// The unit's members that matched the display focus, in member order.
fn focused_members(unit: &ExecutionUnit, targets: &BTreeSet<ModuleKey>) -> String {
    unit.members
        .iter()
        .filter(|member| targets.contains(member))
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Print a module-name list as a titled table.
fn print_module_table(title: &str, modules: Vec<String>) {
    let mut table = OutputTable::new(vec!["Module"]).with_title(title);
    for module in modules {
        table.add_row(vec![module]);
    }
    println!("{table}");
}

/// A discovered module paired with its owning workspace, for the `modules`
/// projection. Serializes directly as the stable `jsonl` record schema.
#[derive(serde::Serialize)]
struct ModuleRow {
    module: String,
    workspace: Option<String>,
}

/// The sorted module set with each module's owning workspace.
fn module_rows(graph: &Graph) -> Vec<ModuleRow> {
    graph
        .modules()
        .map(|module| ModuleRow {
            module: module.key().to_string(),
            workspace: module.workspace.as_ref().map(ToString::to_string),
        })
        .collect()
}

/// Render the module set as a titled table with a Workspace column.
fn render_modules_human(rows: &[ModuleRow]) {
    let mut table = OutputTable::new(vec!["Module", "Workspace"]).with_title("Modules");
    for row in rows {
        table.add_row(vec![
            row.module.clone(),
            row.workspace.clone().unwrap_or_default(),
        ]);
    }
    println!("{table}");
}

/// Render the module set as one JSON object per module line (a stable schema).
///
/// # Errors
/// Propagates a serialization failure (never expected for these plain fields).
fn render_modules_jsonl(rows: &[ModuleRow]) -> AppResult<()> {
    for row in rows {
        let line = serde_json::to_string(row).map_err(AppError::internal)?;
        println!("{line}");
    }
    Ok(())
}

/// The sorted, de-duplicated module set referenced by a plan's units.
fn plan_module_names(plan: &Plan) -> Vec<String> {
    let mut modules: Vec<String> = plan
        .units
        .iter()
        .flat_map(|unit| unit.members.iter().map(ToString::to_string))
        .collect();
    modules.sort_unstable();
    modules.dedup();
    modules
}

/// Render the discovered dependency edges as an indented text adjacency list.
fn render_graph_text(graph: &Graph) -> String {
    let mut out = String::new();
    for module in graph.modules() {
        out.push_str(&module.key().to_string());
        out.push('\n');
        for edge in graph
            .edges()
            .iter()
            .filter(|edge| edge.from == module.key())
        {
            out.push_str("  -> ");
            out.push_str(&edge.to.to_string());
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use toven_model::{DepKind, EcosystemId, Edge, Graph, MemberId, Module, ModuleRef, RepoPath};

    use super::{ModuleRow, module_rows, render_graph_text};

    fn module(name: &str) -> Module {
        Module::new(mref(name), RepoPath::new(format!("crates/{name}")).unwrap())
    }

    fn module_keys(rows: &[ModuleRow]) -> Vec<&str> {
        rows.iter().map(|row| row.module.as_str()).collect()
    }

    fn mref(name: &str) -> ModuleRef {
        ModuleRef::new(EcosystemId::new("rust").unwrap(), name).unwrap()
    }

    fn graph() -> Graph {
        Graph::build(
            vec![module("core"), module("app")],
            vec![Edge::new(mref("app"), mref("core"), DepKind::Normal)],
        )
        .expect("valid graph")
    }

    fn federated_graph() -> Graph {
        let mut core = module("core");
        core.member = Some(MemberId::new("core").unwrap());
        let mut app = module("app");
        app.member = Some(MemberId::new("gateway").unwrap());
        let edge = Edge::new(app.key(), core.key(), DepKind::Overlay);
        Graph::build(vec![core, app], vec![edge]).expect("valid federated graph")
    }

    #[test]
    fn module_names_are_sorted_and_deduplicated() {
        assert_eq!(
            module_keys(&module_rows(&graph())),
            vec!["rust:app", "rust:core"]
        );
    }

    #[test]
    fn module_names_include_member_scope_when_present() {
        assert_eq!(
            module_keys(&module_rows(&federated_graph())),
            vec!["core/rust:core", "gateway/rust:app"]
        );
    }

    #[test]
    fn module_rows_carry_the_owning_workspace() {
        use toven_model::WorkspaceId;
        let mut core = module("core");
        core.workspace = Some(WorkspaceId::new("core").unwrap());
        let app = module("app");
        let graph = Graph::build(vec![core, app], Vec::new()).expect("valid graph");
        let rows = module_rows(&graph);
        assert_eq!(rows[0].module, "rust:app");
        assert_eq!(rows[0].workspace, None);
        assert_eq!(rows[1].module, "rust:core");
        assert_eq!(rows[1].workspace.as_deref(), Some("core"));
    }

    #[test]
    fn module_row_jsonl_is_a_bespoke_record_not_an_event() {
        // `modules --output jsonl` is a stable domain schema on stdout, one
        // record per row — deliberately *not* routed through the Event stream.
        // Guard that contract: the record must never carry an `event`
        // discriminator, and its shape stays exactly `{module, workspace}`.
        let row = ModuleRow {
            module: "rust:core".to_string(),
            workspace: Some("core".to_string()),
        };
        let value = serde_json::to_value(&row).expect("serialize");
        let object = value.as_object().expect("object");
        assert!(object.get("event").is_none(), "not an Event record");
        assert_eq!(object.len(), 2);
        assert_eq!(value["module"], "rust:core");
        assert_eq!(value["workspace"], "core");
    }

    #[test]
    fn graph_text_lists_each_module_and_its_edges() {
        let rendered = render_graph_text(&graph());
        assert!(rendered.contains("rust:app"));
        assert!(rendered.contains("  -> rust:core"));
    }

    #[test]
    fn graph_text_uses_member_scoped_node_names() {
        let rendered = render_graph_text(&federated_graph());
        assert!(rendered.contains("gateway/rust:app"));
        assert!(rendered.contains("  -> core/rust:core"));
    }

    #[test]
    fn plan_module_names_expand_batched_members() {
        use toven_model::{CacheVerdict, ExecutionReadiness, ExecutionUnit, ModuleKey, Plan};
        let unit = ExecutionUnit {
            id: "rust#test".to_string(),
            module: ModuleKey::bare(mref("app")),
            members: vec![ModuleKey::bare(mref("app")), ModuleKey::bare(mref("core"))],
            task: "test".to_string(),
            origin: toven_model::TaskOrigin::AdapterDefault,
            workspace: None,
            argv: vec!["cargo".to_string(), "test".to_string()],
            persistent: false,
            readiness: ExecutionReadiness::Started,
            readiness_timeout: std::time::Duration::from_secs(30),
            fail_if_output: false,
            cache: CacheVerdict::Miss,
            cache_key: None,
            depends_on: Vec::new(),
            resource_group: None,
        };
        let plan = Plan::new(vec![unit], vec![vec!["rust#test".to_string()]]);
        assert_eq!(
            super::plan_module_names(&plan),
            vec!["rust:app", "rust:core"]
        );
    }
}
