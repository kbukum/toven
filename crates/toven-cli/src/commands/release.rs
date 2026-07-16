//! The `release` lifecycle verb: `plan`, `status`, `tag`, `publish`.
//!
//! `plan` and `status` are read-only projections over the engine release spine —
//! they render typed data on stdout and never mutate a manifest, tag, or
//! registry. `tag` and `publish` drive the mutating release pipeline
//! ([`release_run`]): `tag` stops after the release commit/tag/push, `publish`
//! continues to the registry. `publish` under `--dry-run` instead runs a
//! no-mutation rehearsal ([`release_rehearse`]) that reports the resolved publish
//! order and per-module would-publish/already-published verdicts. Libraries return typed data;
//! this CLI layer is the only one that prints, following the introspection stream
//! convention (projection on stdout, warnings/summaries on stderr).

use rskit_cli::{ExitCode, OutputKV, OutputTable};
use rskit_errors::{AppError, AppResult};
use serde::Serialize;
use toven_engine::plan::PlanRequest;
use toven_engine::release::{
    PublishDecision, ReleaseApplyOptions, ReleasePlan, ReleaseRehearsal, ReleaseStatus,
    release_plan, release_rehearse, release_run, release_status,
};
use toven_engine::vcs::BaselineFlags;
use toven_model::Event;
use toven_ports::{Provider, Reporter, TaskIntent};

use crate::flags::{Cli, OutputKind, ReleaseAction};
use crate::host::{Project, Report, new_run_id, resolve_output};

/// A quiet [`Reporter`] for the read-only release projections: the projection
/// itself is the stdout payload, so only warnings are surfaced (on stderr).
struct QuietReporter;

impl Reporter for QuietReporter {
    fn emit(&mut self, event: &Event) -> AppResult<()> {
        if let Event::Warning { message } = event {
            eprintln!("warning: {message}");
        }
        Ok(())
    }
}

/// Dispatch a `toven release <action>` invocation.
///
/// # Errors
/// Propagates release PLAN failures (configuration, discovery, graph) and, for
/// the mutating actions, APPLY failures (guardrails, mutation, tagging,
/// publishing).
pub(crate) fn execute(
    providers: &[&dyn Provider],
    project: &Project,
    cli: &Cli,
    action: ReleaseAction,
) -> AppResult<ExitCode> {
    match action {
        ReleaseAction::Plan => plan(providers, project, cli.output),
        ReleaseAction::Status => status(providers, project, cli.output),
        ReleaseAction::Publish if cli.dry_run => rehearse(providers, project, cli.output),
        ReleaseAction::Tag | ReleaseAction::Publish => run(providers, project, cli, action),
    }
}

/// Build the release-scoped PLAN request rooted at the project.
fn release_request(project: &Project) -> AppResult<PlanRequest> {
    Ok(PlanRequest::new(
        new_run_id()?,
        project.document.project.name.clone(),
        TaskIntent::resolve("release"),
        project.project_root.clone(),
    ))
}

/// `release plan`: render the release PLAN cut without mutating anything.
fn plan(
    providers: &[&dyn Provider],
    project: &Project,
    output: Option<OutputKind>,
) -> AppResult<ExitCode> {
    let request = release_request(project)?;
    let opened = project.open_member_vcs(providers, &BaselineFlags::new())?;
    let readers = opened.readers();
    let mut reporter = QuietReporter;
    let plan = release_plan(
        &request,
        &project.document,
        providers,
        &readers,
        &mut reporter,
    )?;
    match resolve_output(output, &project.document) {
        OutputKind::Jsonl => render_plan_jsonl(&plan)?,
        OutputKind::Human => render_plan_human(&plan),
    }
    Ok(ExitCode::Success)
}

/// `release status`: render each module's declared/published/tagged state.
fn status(
    providers: &[&dyn Provider],
    project: &Project,
    output: Option<OutputKind>,
) -> AppResult<ExitCode> {
    let request = release_request(project)?;
    let opened = project.open_member_vcs(providers, &BaselineFlags::new())?;
    let readers = opened.readers();
    let mut reporter = QuietReporter;
    let status = release_status(
        &request,
        &project.document,
        providers,
        &readers,
        &mut reporter,
    )?;
    match resolve_output(output, &project.document) {
        OutputKind::Jsonl => render_status_jsonl(&status)?,
        OutputKind::Human => render_status_human(&status),
    }
    Ok(ExitCode::Success)
}

/// `release publish --dry-run`: rehearse the publish loop, mutating nothing.
fn rehearse(
    providers: &[&dyn Provider],
    project: &Project,
    output: Option<OutputKind>,
) -> AppResult<ExitCode> {
    let request = release_request(project)?;
    let opened = project.open_member_vcs(providers, &BaselineFlags::new())?;
    let readers = opened.readers();
    let mut reporter = QuietReporter;
    let rehearsal = release_rehearse(
        &request,
        &project.document,
        providers,
        &readers,
        &mut reporter,
    )?;
    match resolve_output(output, &project.document) {
        OutputKind::Jsonl => render_rehearsal_jsonl(&rehearsal)?,
        OutputKind::Human => render_rehearsal_human(&rehearsal),
    }
    Ok(ExitCode::Success)
}

/// `release tag/publish`: drive the mutating release pipeline. `tag` stops after
/// commit/tag/push; `publish` continues to the registry.
fn run(
    providers: &[&dyn Provider],
    project: &Project,
    cli: &Cli,
    action: ReleaseAction,
) -> AppResult<ExitCode> {
    let request = release_request(project)?;
    let opened = project.open_member_vcs(providers, &BaselineFlags::new())?;
    let readers = opened.readers();
    let repos = opened.release_repos();
    let report = Report::resolve(
        cli.output,
        cli.verbosity(),
        cli.color_choice(),
        &project.document,
    );
    let mut reporter = report.reporter();
    let sink: &mut dyn Reporter = reporter.as_mut();

    let options = ReleaseApplyOptions {
        allow_dirty: cli.allow_dirty,
        push: !cli.no_push,
        publish: matches!(action, ReleaseAction::Publish),
        ..ReleaseApplyOptions::default()
    };
    release_run(
        &request,
        &project.document,
        providers,
        &readers,
        &repos,
        sink,
        &options,
    )?;
    Ok(ExitCode::Success)
}

/// A stable JSON-lines record for one `release plan` entry.
#[derive(Serialize)]
struct PlanRecord {
    module: String,
    current_version: String,
    planned_version: Option<String>,
    publish_needed: bool,
    summary: String,
}

fn render_plan_human(plan: &ReleasePlan) {
    let mut table = OutputTable::new(vec!["Module", "Current", "Planned", "Publish", "Summary"])
        .with_title(format!("Release plan ({})", plan.strategy.as_str()));
    for entry in &plan.entries {
        table.add_row(vec![
            entry.module.to_string(),
            entry.current_version.to_string(),
            entry
                .planned_version
                .as_ref()
                .map_or_else(|| "-".to_string(), ToString::to_string),
            if entry.publish_needed { "yes" } else { "no" }.to_string(),
            entry.changelog.summary.clone(),
        ]);
    }
    println!("{table}");
}

fn render_plan_jsonl(plan: &ReleasePlan) -> AppResult<()> {
    for entry in &plan.entries {
        let record = PlanRecord {
            module: entry.module.to_string(),
            current_version: entry.current_version.to_string(),
            planned_version: entry.planned_version.as_ref().map(ToString::to_string),
            publish_needed: entry.publish_needed,
            summary: entry.changelog.summary.clone(),
        };
        let line = serde_json::to_string(&record).map_err(AppError::internal)?;
        println!("{line}");
    }
    Ok(())
}

/// A stable JSON-lines record for one `release status` module.
#[derive(Serialize)]
struct StatusRecord {
    module: String,
    declared_version: String,
    latest_tag: Option<String>,
    published_versions: Vec<String>,
    is_published: bool,
}

fn render_status_human(status: &ReleaseStatus) {
    let mut table = OutputTable::new(vec!["Module", "Declared", "Latest tag", "Published"])
        .with_title("Release status");
    for module in &status.modules {
        table.add_row(vec![
            module.module.to_string(),
            module.declared_version.to_string(),
            module.latest_tag.clone().unwrap_or_else(|| "-".to_string()),
            if module.is_published { "yes" } else { "no" }.to_string(),
        ]);
    }
    println!("{table}");
}

fn render_status_jsonl(status: &ReleaseStatus) -> AppResult<()> {
    for module in &status.modules {
        let record = StatusRecord {
            module: module.module.to_string(),
            declared_version: module.declared_version.to_string(),
            latest_tag: module.latest_tag.clone(),
            published_versions: module
                .published_versions
                .iter()
                .map(ToString::to_string)
                .collect(),
            is_published: module.is_published,
        };
        let line = serde_json::to_string(&record).map_err(AppError::internal)?;
        println!("{line}");
    }
    Ok(())
}

/// A stable JSON-lines record for one rehearsed publish verdict.
#[derive(Serialize)]
struct RehearsalRecord {
    module: String,
    version: String,
    decision: String,
}

fn render_rehearsal_human(rehearsal: &ReleaseRehearsal) {
    let would_publish = rehearsal
        .verdicts
        .iter()
        .filter(|verdict| verdict.decision == PublishDecision::WouldPublish)
        .count();
    let already_published = rehearsal.verdicts.len() - would_publish;
    let mut summary = OutputKV::new();
    summary
        .add("strategy", rehearsal.strategy.as_str().to_string())
        .add("would_publish", would_publish.to_string())
        .add("already_published", already_published.to_string());
    println!("{summary}");
    let mut table = OutputTable::new(vec!["Module", "Version", "Decision"])
        .with_title("Release rehearsal (no mutation)");
    for verdict in &rehearsal.verdicts {
        table.add_row(vec![
            verdict.module.to_string(),
            verdict.version.to_string(),
            verdict.decision.as_str().to_string(),
        ]);
    }
    println!("{table}");
}

fn render_rehearsal_jsonl(rehearsal: &ReleaseRehearsal) -> AppResult<()> {
    for verdict in &rehearsal.verdicts {
        let record = RehearsalRecord {
            module: verdict.module.to_string(),
            version: verdict.version.to_string(),
            decision: verdict.decision.as_str().to_string(),
        };
        let line = serde_json::to_string(&record).map_err(AppError::internal)?;
        println!("{line}");
    }
    Ok(())
}
