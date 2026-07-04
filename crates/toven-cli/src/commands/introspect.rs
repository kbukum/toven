//! Introspection verbs: `modules`/`list`, `graph`/`deps`, `affected`, and
//! `explain`.
//!
//! `affected` and `explain` are thin projections over one immutable [`Plan`].
//! The shared [`build_plan`] runs the PLAN spine once with caching disabled
//! (introspection never executes, so cache verdicts are noise) and a reporter that
//! keeps stdout reserved for the projection while still surfacing warnings, then
//! the verb filters the resulting units.
//!
//! `modules` and `graph` project the validated discovered [`Graph`] directly, so
//! they do not depend on any particular task kind being configured or schedulable.

use rskit_cli::{ExitCode, OutputKV, OutputTable};
use rskit_errors::{AppError, AppResult};
use toven_engine::federation::resolve::PathDriverLocator;
use toven_engine::plan::{
    CacheMode, FsSourceDigest, NullCache, PlanHost, PlanRequest, ProcessToolchainProber, Selection,
    dependency_graph, plan,
};
use toven_engine::vcs::BaselineFlags;
use toven_model::{Event, Graph, Plan};
use toven_ports::{Provider, Reporter, TaskKind};

use crate::commands::selection::TaskSelection;
use crate::flags::GraphFormat;
use crate::host::{Project, new_run_id};

/// A quiet [`Reporter`]: introspection prints its projection on stdout, while
/// warnings still go to stderr so warn-and-skip diagnostics are visible.
struct QuietReporter;

impl Reporter for QuietReporter {
    fn emit(&mut self, event: &Event) -> AppResult<()> {
        if let Event::Warning { message } = event {
            eprintln!("warning: {message}");
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
/// Propagates PLAN-spine failures (configuration, discovery, graph, scheduling).
fn build_plan(
    providers: &[&dyn Provider],
    project: &Project,
    intent: TaskKind,
    baseline: &BaselineFlags,
    selection: Selection,
) -> AppResult<Plan> {
    let request = PlanRequest::new(
        new_run_id(),
        project.document.project.name.clone(),
        intent,
        project.project_root.clone(),
    )
    .with_cache_mode(CacheMode::Disabled)
    .with_selection(selection);

    let opened = project.open_member_vcs(providers, baseline)?;
    let readers = opened.readers();
    let digest = FsSourceDigest::new(&project.project_root);
    let prober = ProcessToolchainProber::new();
    let cache = NullCache;
    let host = PlanHost::new(&readers, &digest, &prober, &cache);

    let mut reporter = QuietReporter;
    plan(&request, &project.document, providers, host, &mut reporter)
}

/// Build the validated discovered module graph for topology introspection verbs.
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

/// `toven modules` / `list` / `ls`: the discovered module set.
///
/// # Errors
/// Propagates [`build_graph`] failures.
pub(crate) fn modules(providers: &[&dyn Provider], project: &Project) -> AppResult<ExitCode> {
    let graph = build_graph(providers, project)?;
    print_module_table("Modules", graph_module_names(&graph));
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
    selection: &TaskSelection,
) -> AppResult<ExitCode> {
    let resolved = selection.resolve(project.document.project.base_ref.as_deref())?;
    let plan = build_plan(providers, project, intent, &selection.baseline, resolved)?;
    print_module_table("Affected", plan_module_names(&plan));
    Ok(ExitCode::Success)
}

/// `toven graph` / `deps`: the discovered dependency edges (`--format text|dot`).
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
        GraphFormat::Dot => render_graph_dot(&graph),
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
    let plan = build_plan(
        providers,
        project,
        intent,
        &BaselineFlags::new(),
        Selection::All,
    )?;
    let mut matched = 0_usize;
    for unit in plan
        .units
        .iter()
        .filter(|unit| unit.members.iter().any(|m| m.to_string() == module))
    {
        matched += 1;
        let mut detail = OutputKV::new();
        detail
            .add("unit", unit.id.clone())
            .add("module", module.to_string())
            .add("representative", unit.module.to_string())
            .add(
                "modules",
                unit.members
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
            )
            .add("task", unit.kind.clone())
            .add("origin", unit.origin.as_str().to_string())
            .add("argv", format!("{:?}", unit.argv))
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

/// Print a module-name list as a titled table.
fn print_module_table(title: &str, modules: Vec<String>) {
    let mut table = OutputTable::new(vec!["Module"]).with_title(title);
    for module in modules {
        table.add_row(vec![module]);
    }
    println!("{table}");
}

/// The sorted module set referenced by a graph.
fn graph_module_names(graph: &Graph) -> Vec<String> {
    graph
        .modules()
        .map(|module| module.key().to_string())
        .collect()
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

/// Render the discovered dependency edges as indented adjacency text.
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

/// Render the discovered dependency edges as a Graphviz DOT digraph.
fn render_graph_dot(graph: &Graph) -> String {
    let mut out = String::from("digraph toven {\n");
    for module in graph.modules() {
        out.push_str("  \"");
        out.push_str(&dot_id(&module.key().to_string()));
        out.push_str("\";\n");
    }
    for edge in graph.edges() {
        out.push_str("  \"");
        out.push_str(&dot_id(&edge.from.to_string()));
        out.push_str("\" -> \"");
        out.push_str(&dot_id(&edge.to.to_string()));
        out.push_str("\";\n");
    }
    out.push_str("}\n");
    out
}

/// Escape a DOT quoted string identifier.
fn dot_id(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use toven_model::{DepKind, EcosystemId, Edge, Graph, MemberId, Module, ModuleRef, RepoPath};

    use super::{dot_id, graph_module_names, render_graph_dot, render_graph_text};

    fn module(name: &str) -> Module {
        Module::new(mref(name), RepoPath::new(format!("crates/{name}")).unwrap())
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
        assert_eq!(graph_module_names(&graph()), vec!["rust:app", "rust:core"]);
    }

    #[test]
    fn module_names_include_member_scope_when_present() {
        assert_eq!(
            graph_module_names(&federated_graph()),
            vec!["core/rust:core", "gateway/rust:app"]
        );
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
    fn graph_dot_emits_a_digraph_with_directed_edges() {
        let rendered = render_graph_dot(&graph());
        assert!(rendered.starts_with("digraph toven {"));
        assert!(rendered.contains("\"rust:app\" -> \"rust:core\";"));
        assert!(rendered.trim_end().ends_with('}'));
    }

    #[test]
    fn graph_dot_uses_member_scoped_node_names() {
        let rendered = render_graph_dot(&federated_graph());
        assert!(rendered.contains("\"gateway/rust:app\" -> \"core/rust:core\";"));
    }

    #[test]
    fn graph_dot_escapes_quoted_identifiers() {
        assert_eq!(dot_id("rust:app\"#build\\dev"), "rust:app\\\"#build\\\\dev");
    }

    #[test]
    fn plan_module_names_expand_batched_members() {
        use toven_model::{CacheVerdict, ExecutionReadiness, ExecutionUnit, ModuleKey, Plan};
        let unit = ExecutionUnit {
            id: "rust#test".to_string(),
            module: ModuleKey::bare(mref("app")),
            members: vec![ModuleKey::bare(mref("app")), ModuleKey::bare(mref("core"))],
            kind: "test".to_string(),
            origin: toven_model::TaskOrigin::AdapterDefault,
            workspace: None,
            argv: vec!["cargo".to_string(), "test".to_string()],
            persistent: false,
            readiness: ExecutionReadiness::Started,
            readiness_timeout: std::time::Duration::from_secs(30),
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
