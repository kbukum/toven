//! The read-only checks: `status` and `readiness`, with their projectors.

use rskit_cli::ExitCode;
use rskit_errors::{AppError, AppResult};
use serde::Serialize;
use std::sync::Arc;
use toven_core::vcs::BaselineFlags;
use toven_exec::ProcessSupervisor;
use toven_ports::Provider;
use toven_release::{ReadinessCheck, ReleaseModuleStatus, readiness_operation, status_operation};

use crate::commands::support::QuietReporter;
use crate::flags::{Cli, OutputKind};
use crate::host::{Project, resolve_output};

use super::dispatch::{drive_streamed, engine_jobs, release_request};
use super::summary::{publication_label, status_flow_label};

/// `release status`: render each module's declared/published/tagged state.
pub(super) fn status(
    providers: &[&dyn Provider],
    supervisor: &Arc<ProcessSupervisor>,
    project: &Project,
    cli: &Cli,
) -> AppResult<ExitCode> {
    let request = release_request(project)?;
    let opened = project.open_member_vcs(providers, &BaselineFlags::new())?;
    let readers = opened.readers();
    let mut reporter = QuietReporter;
    let (operation, units) = status_operation(
        &request,
        &project.document,
        providers,
        &readers,
        &mut reporter,
    )?;

    let output = resolve_output(cli.execution.output, &project.document);
    // Independent modules — one bounded-parallel wave.
    let jobs = engine_jobs(cli, project);
    let mut projector = StatusProjector::new(output);
    projector.open();

    let (summary, cancelled) = drive_streamed(&units, operation, jobs, supervisor, &mut projector)?;
    if cancelled {
        return Ok(ExitCode::Cancelled);
    }

    Ok(if summary.has_failures() {
        ExitCode::Failure
    } else {
        ExitCode::Success
    })
}

/// `release readiness`: stream the fail-closed go/no-go preflight, mutating
/// nothing. Each composed check streams its verdict as it settles; the go/no-go
/// conclusion is assembled from the streamed verdicts. A no-go verdict exits
/// non-zero so CI gates on it.
pub(super) fn readiness(
    providers: &[&dyn Provider],
    supervisor: &Arc<ProcessSupervisor>,
    project: &Project,
    cli: &Cli,
) -> AppResult<ExitCode> {
    let request = release_request(project)?;
    let opened = project.open_member_vcs(providers, &BaselineFlags::new())?;
    let readers = opened.readers();
    let mut reporter = QuietReporter;
    let (operation, units) = readiness_operation(
        &request,
        &project.document,
        providers,
        &readers,
        &mut reporter,
    )?;

    let output = resolve_output(cli.execution.output, &project.document);
    let jobs = engine_jobs(cli, project);
    let mut projector = ReadinessProjector::new(output);
    projector.open();

    if units.is_empty() {
        // No composed checks: state the absence explicitly (an empty check list
        // reads as broken output and a silent "go" is false confidence).
        projector.note_no_checks();
    } else {
        // A failing check is a settled verdict, not a unit error, so the engine
        // runs every check; only I/O or an unknown check errors out here.
        let (_summary, cancelled) =
            drive_streamed(&units, operation, jobs, supervisor, &mut projector)?;
        if cancelled {
            return Ok(ExitCode::Cancelled);
        }
    }

    // Fail closed: any failing check makes the verdict no-go.
    let is_go = projector.is_go();
    projector.verdict(is_go);
    Ok(if is_go {
        ExitCode::Success
    } else {
        ExitCode::Failure
    })
}

/// A stable JSON-lines record for one `release status` module.
#[derive(Serialize)]
pub(super) struct StatusRecord {
    module: String,
    publication: String,
    registry: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    declared_version: Option<String>,
    latest_tag: Option<String>,
    host_forge: Option<String>,
    published_versions: Vec<String>,
    is_published: bool,
    entrypoint: String,
    maintainer_tag_present: Option<bool>,
}

/// Streams `release status` per module as each settles on the runtime engine,
/// never buffering a terminal aggregate before first output.
///
/// Human mode prints one row per module to stdout (a `Release status` header is
/// the deterministic frame; module rows stream in completion order, so goldens
/// match order-insensitively); an `examining` line goes to stderr for live
/// motion. JSONL mode emits one [`StatusRecord`] per module to stdout.
struct StatusProjector {
    pub(super) output: OutputKind,
}

impl StatusProjector {
    pub(super) const fn new(output: OutputKind) -> Self {
        Self { output }
    }

    /// Print the deterministic human header once, before any module streams.
    pub(super) fn open(&self) {
        if matches!(self.output, OutputKind::Human) {
            println!("Release status");
        }
    }

    pub(super) fn render(&self, module: &ReleaseModuleStatus) -> AppResult<()> {
        match self.output {
            OutputKind::Jsonl => {
                let line =
                    serde_json::to_string(&status_record(module)).map_err(AppError::internal)?;
                println!("{line}");
            }
            OutputKind::Human => println!("{}", status_line(module)),
        }
        Ok(())
    }
}

impl toven_runtime::Progress<ReleaseModuleStatus> for StatusProjector {
    fn started(&mut self, unit_id: &str) -> AppResult<()> {
        if matches!(self.output, OutputKind::Human) {
            eprintln!("examining {unit_id}");
        }
        Ok(())
    }

    fn settled(
        &mut self,
        report: &toven_runtime::UnitReport<ReleaseModuleStatus>,
    ) -> AppResult<()> {
        if let Some(module) = &report.outcome {
            self.render(module)?;
        }
        Ok(())
    }
}

/// A compact one-line human projection of a module's release status.
pub(super) fn status_line(module: &ReleaseModuleStatus) -> String {
    format!(
        "{}  {}  declared {}  tag {}  hosted {}  {}  {}",
        module.module,
        publication_label(&module.publication),
        module
            .declared_version
            .as_ref()
            .map_or_else(|| "unreleased".to_owned(), ToString::to_string),
        module.latest_tag.as_deref().unwrap_or("-"),
        module.host_forge.as_deref().unwrap_or("-"),
        status_flow_label(module.entrypoint, module.maintainer_tag_present),
        if module.is_published {
            "published"
        } else {
            "unpublished"
        },
    )
}

/// Project one module's status onto its stable JSON-lines record.
pub(super) fn status_record(module: &ReleaseModuleStatus) -> StatusRecord {
    StatusRecord {
        module: module.module.to_string(),
        publication: module.publication.as_str().to_string(),
        registry: module.publication.registry().map(str::to_string),
        declared_version: module.declared_version.as_ref().map(ToString::to_string),
        latest_tag: module.latest_tag.clone(),
        host_forge: module.host_forge.clone(),
        published_versions: module
            .published_versions
            .iter()
            .map(ToString::to_string)
            .collect(),
        is_published: module.is_published,
        entrypoint: module.entrypoint.as_str().to_string(),
        maintainer_tag_present: module.maintainer_tag_present,
    }
}

/// A stable JSON-lines record for one readiness check.
#[derive(Serialize)]
struct ReadinessRecord {
    check: String,
    passed: bool,
    detail: String,
}

/// Streams `release readiness` per composed check on the runtime engine.
///
/// Human mode prints one line per check to stdout under a `Release readiness`
/// header (checks stream in completion order, so goldens match
/// order-insensitively) and closes with a `verdict:` line; a `checking` line
/// goes to stderr for live motion. JSONL mode emits one [`ReadinessRecord`] per
/// check to stdout (the go/no-go conclusion is conveyed by the exit status). The
/// projector accumulates the settled verdicts so the fail-closed go/no-go is
/// assembled from the streamed outcomes.
struct ReadinessProjector {
    pub(super) output: OutputKind,
    pub(super) checks: Vec<ReadinessCheck>,
}

impl ReadinessProjector {
    pub(super) const fn new(output: OutputKind) -> Self {
        Self {
            output,
            checks: Vec::new(),
        }
    }

    /// Print the deterministic human header once, before any check streams.
    pub(super) fn open(&self) {
        if matches!(self.output, OutputKind::Human) {
            println!("Release readiness");
        }
    }

    /// State the empty-check-list case explicitly (human only).
    pub(super) fn note_no_checks(&self) {
        if matches!(self.output, OutputKind::Human) {
            println!("no readiness checks configured");
        }
    }

    pub(super) fn render(&self, check: &ReadinessCheck) -> AppResult<()> {
        match self.output {
            OutputKind::Jsonl => {
                let record = ReadinessRecord {
                    check: check.name.clone(),
                    passed: check.passed,
                    detail: check.detail.clone(),
                };
                let line = serde_json::to_string(&record).map_err(AppError::internal)?;
                println!("{line}");
            }
            OutputKind::Human => println!("{}", readiness_line(check)),
        }
        Ok(())
    }

    /// Fail closed: a go requires every settled check to have passed.
    pub(super) fn is_go(&self) -> bool {
        self.checks.iter().all(|check| check.passed)
    }

    /// Print the go/no-go conclusion last (human only), after its evidence.
    pub(super) fn verdict(&self, is_go: bool) {
        if matches!(self.output, OutputKind::Human) {
            println!("verdict: {}", if is_go { "go" } else { "no-go" });
        }
    }
}

impl toven_runtime::Progress<ReadinessCheck> for ReadinessProjector {
    fn started(&mut self, unit_id: &str) -> AppResult<()> {
        if matches!(self.output, OutputKind::Human) {
            eprintln!("checking {unit_id}");
        }
        Ok(())
    }

    fn settled(&mut self, report: &toven_runtime::UnitReport<ReadinessCheck>) -> AppResult<()> {
        if let Some(check) = &report.outcome {
            self.render(check)?;
            self.checks.push(check.clone());
        }
        Ok(())
    }
}

/// A compact one-line human projection of one readiness check's verdict.
fn readiness_line(check: &ReadinessCheck) -> String {
    format!(
        "{}  {}  {}",
        check.name,
        if check.passed { "pass" } else { "fail" },
        check.detail,
    )
}
