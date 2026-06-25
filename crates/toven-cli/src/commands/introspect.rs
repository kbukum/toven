//! Introspection verbs: `modules`/`list`, `graph`/`deps`, `affected`, and
//! `explain` (cli-taxonomy Decision D).
//!
//! Each verb is a **thin projection over one immutable [`Plan`]** — there is no
//! second planning path. The shared [`build_plan`] runs the PLAN spine once with
//! caching disabled (introspection never executes, so cache verdicts are noise)
//! and a silent reporter (the projection is the output, not the event stream),
//! then the verb filters and formats the resulting units / dependency edges.
//!
//! `modules`/`graph` have no task argument, so they plan a representative
//! [`TaskKind::Build`] cut to populate the unit set; `affected`/`explain` project
//! the task the user named.

use rskit_cli::{ExitCode, OutputKV, OutputTable};
use rskit_errors::{AppError, AppResult};
use toven_engine::plan::{
    CacheMode, FsSourceDigest, NullCache, PlanHost, PlanRequest, ProcessToolchainProber, plan,
};
use toven_model::{Event, Plan};
use toven_ports::{Provider, Reporter, TaskKind};

use crate::flags::GraphFormat;
use crate::host::{Project, new_run_id};

/// A discarding [`Reporter`]: introspection prints its projection, not the event
/// stream, so the PLAN-spine events are swallowed.
struct SilentReporter;

impl Reporter for SilentReporter {
    fn emit(&mut self, _event: &Event) -> AppResult<()> {
        Ok(())
    }
}

/// Build the single immutable [`Plan`] every introspection verb projects.
///
/// Caching is disabled (no execution happens, so a real cache verdict is
/// irrelevant and the cache port is never consulted) and the reporter is silent.
///
/// # Errors
/// Propagates PLAN-spine failures (configuration, discovery, graph, scheduling).
fn build_plan(providers: &[&dyn Provider], project: &Project, intent: TaskKind) -> AppResult<Plan> {
    let request = PlanRequest::new(
        new_run_id(),
        project.document.project.name.clone(),
        intent,
        project.project_root.clone(),
    )
    .with_cache_mode(CacheMode::Disabled);

    let vcs = project.open_vcs()?;
    let digest = FsSourceDigest::new(&project.project_root);
    let prober = ProcessToolchainProber::new();
    let cache = NullCache;
    let host = PlanHost::new(&vcs, &digest, &prober, &cache);

    let mut reporter = SilentReporter;
    plan(&request, &project.document, providers, host, &mut reporter)
}

/// `toven modules` / `list` / `ls`: the discovered, planned module set.
///
/// # Errors
/// Propagates [`build_plan`] failures.
pub(crate) fn modules(providers: &[&dyn Provider], project: &Project) -> AppResult<ExitCode> {
    let plan = build_plan(providers, project, TaskKind::Build)?;
    print_module_table("Modules", &plan);
    Ok(ExitCode::Success)
}

/// `toven affected <task>`: the modules with a scheduled unit for `task`.
///
/// # Errors
/// Propagates [`build_plan`] failures.
pub(crate) fn affected(
    providers: &[&dyn Provider],
    project: &Project,
    intent: TaskKind,
) -> AppResult<ExitCode> {
    let plan = build_plan(providers, project, intent)?;
    print_module_table("Affected", &plan);
    Ok(ExitCode::Success)
}

/// `toven graph` / `deps`: the scheduled dependency edges (`--format text|dot`).
///
/// # Errors
/// Propagates [`build_plan`] failures.
pub(crate) fn graph(
    providers: &[&dyn Provider],
    project: &Project,
    format: GraphFormat,
) -> AppResult<ExitCode> {
    let plan = build_plan(providers, project, TaskKind::Build)?;
    let rendered = match format {
        GraphFormat::Text => render_graph_text(&plan),
        GraphFormat::Dot => render_graph_dot(&plan),
    };
    print!("{rendered}");
    Ok(ExitCode::Success)
}

/// `toven explain <module> <task>`: the planned units for one module and task.
///
/// # Errors
/// Returns a not-found error when no unit matches the module, else propagates
/// [`build_plan`] failures.
pub(crate) fn explain(
    providers: &[&dyn Provider],
    project: &Project,
    module: &str,
    intent: TaskKind,
) -> AppResult<ExitCode> {
    let plan = build_plan(providers, project, intent)?;
    let mut matched = 0_usize;
    for unit in plan
        .units
        .iter()
        .filter(|unit| unit.module.to_string() == module)
    {
        matched += 1;
        let mut detail = OutputKV::new();
        detail
            .add("unit", unit.id.clone())
            .add("module", unit.module.to_string())
            .add("task", unit.kind.clone())
            .add("argv", unit.argv.join(" "))
            .add("cache", format!("{:?}", unit.cache))
            .add("persistent", unit.persistent.to_string())
            .add("depends_on", unit.depends_on.join(", "));
        println!("{detail}");
    }
    if matched == 0 {
        return Err(AppError::not_found(
            module,
            Some("no planned unit for that module and task"),
        ));
    }
    Ok(ExitCode::Success)
}

/// Print the unique module set of `plan` as a titled table.
fn print_module_table(title: &str, plan: &Plan) {
    let mut table = OutputTable::new(vec!["Module"]).with_title(title);
    for module in module_names(plan) {
        table.add_row(vec![module]);
    }
    println!("{table}");
}

/// The sorted, de-duplicated module set referenced by a plan's units.
fn module_names(plan: &Plan) -> Vec<String> {
    let mut modules: Vec<String> = plan
        .units
        .iter()
        .map(|unit| unit.module.to_string())
        .collect();
    modules.sort_unstable();
    modules.dedup();
    modules
}

/// Render the scheduled dependency edges as indented adjacency text.
fn render_graph_text(plan: &Plan) -> String {
    let mut out = String::new();
    for unit in &plan.units {
        out.push_str(&unit.id);
        out.push('\n');
        for dependency in &unit.depends_on {
            out.push_str("  -> ");
            out.push_str(dependency);
            out.push('\n');
        }
    }
    out
}

/// Render the scheduled dependency edges as a Graphviz DOT digraph.
fn render_graph_dot(plan: &Plan) -> String {
    let mut out = String::from("digraph toven {\n");
    for unit in &plan.units {
        out.push_str("  \"");
        out.push_str(&unit.id);
        out.push_str("\";\n");
        for dependency in &unit.depends_on {
            out.push_str("  \"");
            out.push_str(dependency);
            out.push_str("\" -> \"");
            out.push_str(&unit.id);
            out.push_str("\";\n");
        }
    }
    out.push_str("}\n");
    out
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use toven_model::{
        CacheVerdict, EcosystemId, ExecutionReadiness, ExecutionUnit, ModuleRef, Plan,
    };

    use super::{module_names, render_graph_dot, render_graph_text};

    fn unit(module: &str, depends_on: Vec<&str>) -> ExecutionUnit {
        ExecutionUnit {
            id: format!("rust:{module}#build"),
            module: ModuleRef::new(EcosystemId::new("rust").unwrap(), module).unwrap(),
            kind: "build".to_string(),
            workspace: None,
            argv: vec!["cargo".to_string(), "build".to_string()],
            persistent: false,
            readiness: ExecutionReadiness::Started,
            readiness_timeout: Duration::from_secs(30),
            cache: CacheVerdict::Miss,
            cache_key: None,
            depends_on: depends_on.into_iter().map(str::to_string).collect(),
            resource_group: None,
        }
    }

    fn plan() -> Plan {
        Plan::new(
            vec![
                unit("core", Vec::new()),
                unit("app", vec!["rust:core#build"]),
            ],
            Vec::new(),
        )
    }

    #[test]
    fn module_names_are_sorted_and_deduplicated() {
        let plan = Plan::new(
            vec![
                unit("app", Vec::new()),
                unit("core", Vec::new()),
                unit("app", Vec::new()),
            ],
            Vec::new(),
        );
        assert_eq!(module_names(&plan), vec!["rust:app", "rust:core"]);
    }

    #[test]
    fn graph_text_lists_each_unit_and_its_edges() {
        let rendered = render_graph_text(&plan());
        assert!(rendered.contains("rust:app#build"));
        assert!(rendered.contains("  -> rust:core#build"));
    }

    #[test]
    fn graph_dot_emits_a_digraph_with_directed_edges() {
        let rendered = render_graph_dot(&plan());
        assert!(rendered.starts_with("digraph toven {"));
        assert!(rendered.contains("\"rust:core#build\" -> \"rust:app#build\";"));
        assert!(rendered.trim_end().ends_with('}'));
    }
}
