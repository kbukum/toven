//! The `coverage` verb: run the coverage task, aggregate its emitted profiles
//! per module, and gate them against the resolved `[…coverage]` thresholds.
//!
//! Coverage is a recognized task kind: the ecosystem tool measures (cargo
//! llvm-cov lcov / Go `-coverprofile`), staging its profiles under
//! [`COVERAGE_DIR`]; Toven then aggregates and decides the pass/fail verdict.
//! This verb runs the `coverage` task through the human run reporter (progress
//! and summary on stderr), then emits one `ModuleCoverageFinished` event per
//! module (one JSONL record each under `--output jsonl`), closes with the tally
//! on stderr, and exits non-zero when the gate fails closed.
//! `--line`/`--function`/`--region`/`--changed-line`/`--enforcement` layer over
//! the config defaults for the run (argv wins; config is the default), and the
//! selection flags narrow the measured scope exactly as the task verbs do.

use std::sync::Arc;

use rskit_cli::{ExitCode, Tone};
use rskit_errors::AppResult;
use toven_core::config::ViewMode;
use toven_core::plan::PlanRequest;
use toven_engine::coverage::{COVERAGE_DIR, CoverageOverrides, CoverageReport, coverage_report};
use toven_exec::ProcessSupervisor;
use toven_model::OutcomeSummary;
use toven_ports::{Provider, TaskIntent};

use crate::commands::run::WatchFlags;
use crate::commands::selection::TaskSelection;
use crate::flags::{Cli, DEFAULT_WATCH_DEBOUNCE_MS, OutputKind};
use crate::host::{Project, Report, new_run_id, resolve_output};
use crate::report::stderr_theme;

/// The recognized task name the coverage verb runs and gates.
const COVERAGE_TASK: &str = "coverage";

/// Dispatch `toven coverage`: measure, aggregate, gate, and report.
///
/// # Errors
/// Propagates the coverage task's PLAN/APPLY failures, VCS I/O failures, an
/// invalid ecosystem coverage config, and a profile read/parse error.
pub(crate) fn execute(
    providers: &[&dyn Provider],
    supervisor: &Arc<ProcessSupervisor>,
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

    let measured = measure(providers, supervisor, project, cli, &selection)?;
    let output = resolve_output(cli.output, &project.document);
    // In human mode the per-module verdicts render as an indented list under
    // this header; JSONL mode stays events-only.
    if matches!(output, OutputKind::Human) {
        eprintln!("{}", stderr_theme(cli.color_choice()).heading("Coverage"));
    }
    let report = stream(providers, project, cli, &selection, &overrides)?;

    // In human mode, close with the tally on stderr alongside the measurement's
    // run summary; JSONL mode stays events-only. The tone tracks the gate verdict
    // so a failing gate never renders as visually successful.
    if matches!(output, OutputKind::Human) {
        let tone = if report.gate_passed() {
            Tone::Success
        } else {
            Tone::Error
        };
        eprintln!(
            "{}",
            stderr_theme(cli.color_choice()).action("Finished", &summary_line(&report), tone,)
        );
    }

    // Derive the exit once from the item-based summary (the gate verdict) folded
    // with the measurement's own outcome.
    let summary = coverage_outcome(&report);
    Ok(
        if matches!(measured, ExitCode::Success) && !summary.has_failures() {
            ExitCode::Success
        } else {
            ExitCode::Failure
        },
    )
}

/// Map the gated coverage report onto the shared item-based summary: each
/// passed module is a `succeeded` item, each failed module a `failed` one, and
/// advisory/excluded modules are `skipped` (measured but not gate failures). The
/// process exit derives solely from this summary's [`OutcomeSummary::has_failures`].
fn coverage_outcome(report: &CoverageReport) -> OutcomeSummary {
    let tally = report.tally();
    OutcomeSummary {
        processed: report.modules.len(),
        succeeded: tally.passed,
        failed: tally.failed,
        skipped: tally.advisory + tally.excluded,
    }
}

/// Run the recognized `coverage` task to emit the profiles, streaming its
/// progress through the human reporter on stderr (never stdout, which the
/// verdict table owns).
fn measure(
    providers: &[&dyn Provider],
    supervisor: &Arc<ProcessSupervisor>,
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
        supervisor,
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
/// measurement ran under. `coverage_report` emits one
/// [`Event::ModuleCoverageFinished`](toven_model::Event::ModuleCoverageFinished)
/// per module (one JSONL record each under `--output jsonl`); the returned
/// report drives the closing summary and the summary-based exit.
fn stream(
    providers: &[&dyn Provider],
    project: &Project,
    cli: &Cli,
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
    let report = Report::resolve(
        cli.output,
        cli.verbosity(),
        cli.color_choice(),
        &project.document,
    );
    let mut reporter = report.reporter();
    coverage_report(
        &request,
        &project.document,
        providers,
        &readers,
        reporter.as_mut(),
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

/// Build the one-line verdict summary rendered on stderr.
///
/// Only the non-zero groups appear so the tally stays scannable, closing with
/// the gate verdict that matches the summary-derived exit.
fn summary_line(report: &CoverageReport) -> String {
    let tally = report.tally();
    let mut parts = Vec::new();
    if tally.passed > 0 {
        parts.push(format!("{} passed", tally.passed));
    }
    if tally.failed > 0 {
        parts.push(format!("{} failed", tally.failed));
    }
    if tally.advisory > 0 {
        parts.push(format!("{} advisory", tally.advisory));
    }
    if tally.excluded > 0 {
        parts.push(format!("{} excluded", tally.excluded));
    }
    if parts.is_empty() {
        parts.push("no modules measured".to_string());
    }
    format!(
        "coverage: {} — {}",
        parts.join(", "),
        if report.gate_passed() {
            "gate passed"
        } else {
            "gate failed"
        }
    )
}

#[cfg(test)]
mod tests {
    use super::{coverage_outcome, summary_line};
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
    fn summary_reports_the_tally_and_gate_verdict() {
        let summary = summary_line(&report());
        assert!(summary.contains("1 passed, 1 failed"), "{summary}");
        assert!(summary.contains("gate failed"), "{summary}");
    }

    #[test]
    fn summary_collapses_zero_count_groups() {
        // An all-passing run names only the non-zero group so the tally stays
        // scannable — no `0 failed, 0 advisory, 0 excluded` noise.
        let all_passing = CoverageReport {
            modules: vec![
                module("core", 92.0, ModuleStatus::Passed),
                module("cli", 91.0, ModuleStatus::Passed),
            ],
            changed: false,
        };
        assert_eq!(
            summary_line(&all_passing),
            "coverage: 2 passed — gate passed"
        );
    }

    #[test]
    fn outcome_maps_verdicts_onto_the_item_summary_and_fails_closed() {
        // The gate verdict flows through the shared item-based summary: a passed
        // module is a succeeded item, a failed module makes `has_failures` true so
        // the exit is derived closed — never from an individual event.
        let summary = coverage_outcome(&report());
        assert_eq!(summary.processed, 2);
        assert_eq!(summary.succeeded, 1);
        assert_eq!(summary.failed, 1);
        assert!(
            summary.has_failures(),
            "a below-threshold module fails closed"
        );
    }

    #[test]
    fn advisory_and_excluded_modules_are_skipped_not_failures() {
        // Advisory/excluded modules are measured but never gate failures, so they
        // count as skipped and leave the summary passing.
        let report = CoverageReport {
            modules: vec![
                module("core", 92.0, ModuleStatus::Passed),
                module("cli", 40.0, ModuleStatus::Advisory),
                module("api", 10.0, ModuleStatus::Excluded),
            ],
            changed: false,
        };
        let summary = coverage_outcome(&report);
        assert_eq!(summary.succeeded, 1);
        assert_eq!(summary.skipped, 2);
        assert_eq!(summary.failed, 0);
        assert!(!summary.has_failures());
    }
}
