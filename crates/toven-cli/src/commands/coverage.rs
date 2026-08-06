//! The `coverage` verb: run the coverage task, aggregate its emitted profiles
//! per module, and gate them against the resolved `[…coverage]` thresholds.
//!
//! Coverage is a recognized task kind: the ecosystem tool measures (cargo
//! llvm-cov lcov / Go `-coverprofile`), staging its profiles under
//! [`COVERAGE_DIR`]; Toven then aggregates and decides the pass/fail verdict.
//! This verb runs the `coverage` task through the human run reporter (progress
//! and summary on stderr), then renders the per-module verdict table on stdout
//! and exits non-zero when the gate fails closed. `--line`/`--function`/
//! `--region`/`--changed-line`/`--enforcement` layer over the config defaults
//! for the run (argv wins; config is the default), and the selection flags
//! narrow the measured scope exactly as the task verbs do.

use rskit_cli::{ExitCode, OutputTable};
use rskit_errors::{AppError, AppResult};
use serde::Serialize;
use toven_engine::coverage::{
    COVERAGE_DIR, CoverageOverrides, CoverageReport, ModuleCoverage, coverage_report,
};
use toven_engine_core::config::ViewMode;
use toven_engine_core::plan::PlanRequest;
use toven_model::Event;
use toven_ports::{Provider, Reporter, TaskIntent};

use crate::commands::run::WatchFlags;
use crate::commands::selection::TaskSelection;
use crate::flags::{Cli, DEFAULT_WATCH_DEBOUNCE_MS, OutputKind};
use crate::host::{Project, Report, new_run_id, resolve_output};

/// The recognized task name the coverage verb runs and gates.
const COVERAGE_TASK: &str = "coverage";

/// A quiet [`Reporter`] for the aggregation pass: the verdict table is the
/// stdout payload, so only warnings are surfaced (on stderr).
struct QuietReporter;

impl Reporter for QuietReporter {
    fn emit(&mut self, event: &Event) -> AppResult<()> {
        if let Event::Warning { message } = event {
            eprintln!("warning: {message}");
        }
        Ok(())
    }
}

/// Dispatch `toven coverage`: measure, aggregate, gate, and report.
///
/// # Errors
/// Propagates the coverage task's PLAN/APPLY failures, VCS I/O failures, an
/// invalid ecosystem coverage config, and a profile read/parse error.
pub(crate) fn execute(
    providers: &[&dyn Provider],
    project: &Project,
    cli: &Cli,
) -> AppResult<ExitCode> {
    let selection = coverage_selection(cli);
    let overrides = build_overrides(cli);

    // The task's argv writes its profiles into the Toven-owned staging dir. Clear
    // it first so aggregation gates only this run's profiles — a stale profile from
    // an earlier run or a broader selection must not be re-attributed into the
    // current verdict — then recreate it so a tool that does not create the parent
    // (e.g. `go test -coverprofile`) can write into it.
    let staging = project.project_root.as_path().join(COVERAGE_DIR);
    rskit_fs::sync_io::dir::remove_all_if_exists(&staging)?;
    rskit_fs::sync_io::dir::create_all(&staging)?;

    let measured = measure(providers, project, cli, &selection)?;
    let report = aggregate(providers, project, &selection, &overrides)?;

    match resolve_output(cli.output, &project.document) {
        OutputKind::Jsonl => render_jsonl(&report)?,
        OutputKind::Human => render_human(&report),
    }

    let gate_ok = report.gate_passed();
    Ok(if matches!(measured, ExitCode::Success) && gate_ok {
        ExitCode::Success
    } else {
        ExitCode::Failure
    })
}

/// Run the recognized `coverage` task to emit the profiles, streaming its
/// progress through the human reporter on stderr (never stdout, which the
/// verdict table owns).
fn measure(
    providers: &[&dyn Provider],
    project: &Project,
    cli: &Cli,
    selection: &TaskSelection,
) -> AppResult<ExitCode> {
    let report = Report::resolve(
        Some(OutputKind::Human),
        cli.verbosity(),
        cli.color_choice(),
        &project.document,
    );
    crate::commands::run::execute(
        providers,
        project,
        report,
        TaskIntent::resolve(COVERAGE_TASK),
        Vec::new(),
        false,
        false,
        false,
        None,
        false,
        WatchFlags {
            enabled: false,
            debounce_ms: DEFAULT_WATCH_DEBOUNCE_MS,
        },
        Some(ViewMode::Stream),
        None,
        selection,
    )
}

/// Aggregate the emitted profiles into the gated report over the same scope the
/// measurement ran under.
fn aggregate(
    providers: &[&dyn Provider],
    project: &Project,
    selection: &TaskSelection,
    overrides: &CoverageOverrides,
) -> AppResult<CoverageReport> {
    let request = PlanRequest::new(
        new_run_id()?,
        project.document.project.name.clone(),
        TaskIntent::resolve(COVERAGE_TASK),
        project.project_root.clone(),
    )
    .with_selection(selection.resolve(project.document.project.base_ref.as_deref())?);
    let opened = project.open_member_vcs(providers, &selection.baseline)?;
    let readers = opened.readers();
    let mut reporter = QuietReporter;
    coverage_report(
        &request,
        &project.document,
        providers,
        &readers,
        &mut reporter,
        overrides,
    )
}

/// Build the coverage selection from the global selection flags.
fn coverage_selection(cli: &Cli) -> TaskSelection {
    TaskSelection {
        baseline: cli.baseline_flags(),
        modules: cli.module.clone(),
        workspaces: cli.workspace.clone(),
        with_dependents: cli.with_dependents,
        with_dependencies: cli.with_dependencies,
    }
}

/// Build the per-run threshold overrides from the parsed coverage argv.
fn build_overrides(cli: &Cli) -> CoverageOverrides {
    CoverageOverrides {
        line: cli.line,
        function: cli.function,
        region: cli.region,
        changed_line: cli.changed_line,
        enforcement: cli.enforcement.map(Into::into),
    }
}

/// Render the measured percentage of a dimension, or `-` when unmeasured.
fn percent(value: Option<f64>) -> String {
    value.map_or_else(|| "-".to_string(), |pct| format!("{pct:.1}%"))
}

fn render_human(report: &CoverageReport) {
    // stdout carries only the verdict table; the summary is a diagnostic that rides
    // stderr alongside the measurement's run summary.
    println!("{}", coverage_table(report));
    eprintln!("{}", summary_line(report));
}

/// Build the per-module verdict table rendered on stdout.
fn coverage_table(report: &CoverageReport) -> OutputTable {
    let mut table = OutputTable::new(vec![
        "Module",
        "Status",
        "Line",
        "Function",
        "Region",
        "Changed",
        "Enforcement",
    ])
    .with_title(if report.changed {
        "Coverage (changed)"
    } else {
        "Coverage"
    });
    for module in &report.modules {
        table.add_row(vec![
            module.module.to_string(),
            module.status.as_str().to_string(),
            percent(Some(module.metrics.line)),
            percent(module.metrics.function),
            percent(module.metrics.region),
            percent(module.metrics.changed_line),
            module.enforcement.as_str().to_string(),
        ]);
    }
    table
}

/// Build the one-line verdict summary rendered on stderr.
fn summary_line(report: &CoverageReport) -> String {
    let tally = report.tally();
    format!(
        "coverage: {} passed, {} failed, {} advisory, {} excluded — {}",
        tally.passed,
        tally.failed,
        tally.advisory,
        tally.excluded,
        if report.gate_passed() {
            "gate passed"
        } else {
            "gate failed"
        }
    )
}

/// A stable JSON-lines record for one module's coverage verdict.
#[derive(Serialize)]
struct ModuleRecord {
    module: String,
    status: String,
    enforcement: String,
    line: f64,
    function: Option<f64>,
    region: Option<f64>,
    changed_line: Option<f64>,
    dimensions: Vec<DimensionRecord>,
}

/// A stable JSON-lines record for one gated dimension.
#[derive(Serialize)]
struct DimensionRecord {
    dimension: String,
    measured: f64,
    threshold: f64,
    passed: bool,
}

fn render_jsonl(report: &CoverageReport) -> AppResult<()> {
    for module in &report.modules {
        let record = module_record(module);
        let line = serde_json::to_string(&record).map_err(AppError::internal)?;
        println!("{line}");
    }
    Ok(())
}

fn module_record(module: &ModuleCoverage) -> ModuleRecord {
    ModuleRecord {
        module: module.module.to_string(),
        status: module.status.as_str().to_string(),
        enforcement: module.enforcement.as_str().to_string(),
        line: module.metrics.line,
        function: module.metrics.function,
        region: module.metrics.region,
        changed_line: module.metrics.changed_line,
        dimensions: module
            .outcomes
            .iter()
            .map(|outcome| DimensionRecord {
                dimension: outcome.dimension.as_str().to_string(),
                measured: outcome.measured,
                threshold: outcome.threshold,
                passed: outcome.passed,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::{coverage_table, summary_line};
    use toven_engine::coverage::{CoverageMetrics, CoverageReport, ModuleCoverage, ModuleStatus};
    use toven_model::{EcosystemId, ModuleKey, ModuleRef};
    use toven_ports::Enforcement;

    fn module(name: &str, line: f64, status: ModuleStatus) -> ModuleCoverage {
        ModuleCoverage {
            module: ModuleKey::bare(
                ModuleRef::new(EcosystemId::new("rust").unwrap(), name).unwrap(),
            ),
            metrics: CoverageMetrics {
                line,
                function: None,
                region: None,
                changed_line: None,
            },
            enforcement: Enforcement::Block,
            outcomes: Vec::new(),
            status,
        }
    }

    fn report() -> CoverageReport {
        CoverageReport {
            modules: vec![
                module("core", 92.0, ModuleStatus::Passed),
                module("cli", 40.0, ModuleStatus::Failed),
            ],
            changed: false,
        }
    }

    #[test]
    fn table_carries_the_per_module_rows_not_the_summary() {
        let rendered = coverage_table(&report()).to_string();
        assert!(rendered.contains("Coverage"), "{rendered}");
        assert!(rendered.contains("rust:core"), "{rendered}");
        assert!(rendered.contains("rust:cli"), "{rendered}");
        assert!(rendered.contains("92.0%"), "{rendered}");
        // The verdict summary belongs on stderr, never in the stdout table.
        assert!(!rendered.contains("gate failed"), "{rendered}");
    }

    #[test]
    fn summary_reports_the_tally_and_gate_verdict() {
        let summary = summary_line(&report());
        assert!(summary.contains("1 passed, 1 failed"), "{summary}");
        assert!(summary.contains("gate failed"), "{summary}");
    }

    #[test]
    fn changed_scope_titles_the_table() {
        let mut report = report();
        report.changed = true;
        assert!(
            coverage_table(&report)
                .to_string()
                .contains("Coverage (changed)")
        );
    }
}
