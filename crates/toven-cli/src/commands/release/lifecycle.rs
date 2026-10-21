//! The version lifecycle actions: `plan`, `bump`, `tag`, `publish` and the
//! `publish --dry-run` rehearsal.

use rskit_cli::{ExitCode, OutputKV, OutputTable};
use rskit_errors::{AppError, AppResult};
use rskit_util::time::Clock;
use serde::Serialize;
use std::sync::Arc;
use toven_core::config::{Document, VerbId};
use toven_core::federation::MemberVcsReaders;
use toven_core::federation::member_repo::MemberReleaseRepos;
use toven_core::plan::PlanRequest;
use toven_core::vcs::BaselineFlags;
use toven_exec::{ProcessSupervisor, ProcessToolRunner};
use toven_ports::{HookRunner, Provider, Reporter, ToolRunner};
use toven_release::{
    BumpOptions, BumpOverrides, PublishDecision, ReleaseApplyOptions, ReleasePlan,
    ReleaseRehearsal, release_bump, release_plan, release_rehearse, release_run,
};

use crate::commands::support::QuietReporter;
use crate::flags::{Cli, ColorWhen, OutputKind, ReleaseAction};
use crate::host::{Project, Report, resolve_output};

use super::dispatch::{build_overrides, release_request};
use super::summary::{
    bump_outcome, plan_outcome, plan_summary_line, print_release_header, print_release_summary,
    publication_label, release_exit, run_outcome, run_summary_line,
};

/// `release plan`: project the release PLAN decision per module without
/// mutating anything — one live `ModuleReleaseResolved` per module (human on
/// stderr, JSONL on stdout), not a terminal table.
pub(super) fn plan(
    providers: &[&dyn Provider],
    project: &Project,
    cli: &Cli,
) -> AppResult<ExitCode> {
    let output = resolve_output(cli.execution.output, &project.document);
    print_release_header("plan", output, cli.execution.color_choice());
    let plan = stream_release_plan(providers, project, cli, &BumpOverrides::new())?;
    print_release_summary(
        &plan_summary_line(&plan),
        output,
        cli.execution.color_choice(),
    );
    Ok(release_exit(&plan_outcome(&plan)))
}

/// Project the release PLAN decisions for the given per-run overrides, mutating
/// nothing. Shared by `release plan` and by the pre-confirmation preview of the
/// mutating version-cut actions: `release_plan` emits one
/// [`Event::ModuleReleaseResolved`](toven_model::Event::ModuleReleaseResolved)
/// per module in plan order — the same projection `--dry-run` emits. Returns the
/// resolved plan so the caller can derive the summary-based exit.
fn stream_release_plan(
    providers: &[&dyn Provider],
    project: &Project,
    cli: &Cli,
    overrides: &BumpOverrides,
) -> AppResult<ReleasePlan> {
    let request = release_request(project)?;
    let opened = project.open_member_vcs(providers, &BaselineFlags::new())?;
    let readers = opened.readers();
    let report = Report::resolve(
        cli.execution.output,
        cli.execution.verbosity(),
        cli.execution.color_choice(),
        &project.document,
    );
    let mut reporter = report.reporter();
    release_plan(
        &request,
        &project.document,
        providers,
        &readers,
        overrides,
        reporter.as_mut(),
    )
}

/// Run the `release bump` mutation through the live `sink`, then derive the
/// summary-based exit. Extracted so the reporter wiring is one testable seam:
/// `release_bump` emits each module's decision (before mutation) then its staged
/// commit (post-transaction) into `sink`, and the exit is derived from the
/// returned report rather than hardcoded.
///
/// # Errors
/// Propagates every `release_bump` failure (configuration, discovery, plan,
/// guardrails, mutation, staging).
#[allow(clippy::too_many_arguments)]
pub(super) fn stream_bump(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    readers: &MemberVcsReaders<'_>,
    repos: &MemberReleaseRepos<'_>,
    overrides: &BumpOverrides,
    sink: &mut dyn Reporter,
    clock: &dyn Clock,
    hooks: &dyn HookRunner,
    options: BumpOptions,
    output: OutputKind,
    color: ColorWhen,
) -> AppResult<ExitCode> {
    print_release_header("bump", output, color);
    let report = release_bump(
        request, document, providers, readers, repos, overrides, sink, clock, hooks, &options,
    )?;
    print_release_summary(
        &run_summary_line("bump", report.modules.len()),
        output,
        color,
    );
    Ok(release_exit(&bump_outcome(&report)))
}

/// `release publish --dry-run`: rehearse the publish loop, mutating nothing.
///
/// Reuses the plan cut's dependency-ordered streaming: driving the rehearsal
/// through the live reporter streams one `module-release-examining` /
/// `module-release-resolved` pair per module in publication order during the
/// slow cascade I/O (baseline resolution, change detection, registry lookups),
/// so the preview shows module-by-module motion instead of hanging silently.
/// The publish verdicts and the hosted-release plan are the rehearsal-specific
/// result, rendered after the streamed decisions.
pub(super) fn rehearse(
    providers: &[&dyn Provider],
    project: &Project,
    cli: &Cli,
) -> AppResult<ExitCode> {
    let request = release_request(project)?;
    let opened = project.open_member_vcs(providers, &BaselineFlags::new())?;
    let readers = opened.readers();
    let overrides = build_overrides(cli)?;
    let output = resolve_output(cli.execution.output, &project.document);
    print_release_header("publish", output, cli.execution.color_choice());
    // Human mode streams the slow dependency-ordered plan cascade live so an
    // operator sees module-by-module motion during the otherwise-silent seconds;
    // jsonl stays one clean record per unit (the publish/hosted verdicts below),
    // so it drives the cut through a quiet reporter and never interleaves the
    // plan's per-module events with those verdicts.
    let mut quiet = QuietReporter;
    let mut live: Option<Box<dyn toven_ports::Reporter>> =
        (output == OutputKind::Human).then(|| {
            Report::resolve(
                cli.execution.output,
                cli.execution.verbosity(),
                cli.execution.color_choice(),
                &project.document,
            )
            .reporter()
        });
    let reporter: &mut dyn toven_ports::Reporter = match live.as_mut() {
        Some(boxed) => boxed.as_mut(),
        None => &mut quiet,
    };
    let rehearsal = release_rehearse(
        &request,
        &project.document,
        providers,
        &readers,
        &overrides,
        reporter,
        cli.no_push,
    )?;
    match output {
        OutputKind::Jsonl => render_rehearsal_jsonl(&rehearsal)?,
        OutputKind::Human => render_rehearsal_human(&rehearsal),
    }
    print_release_summary(
        &rehearsal_summary_line(&rehearsal),
        output,
        cli.execution.color_choice(),
    );
    Ok(ExitCode::Success)
}

/// The closing line for a `release publish --dry-run`, phrased as a rehearsal
/// (nothing was mutated): how many modules *would* publish, plus the tag-only
/// and already-published tallies when non-zero.
fn rehearsal_summary_line(rehearsal: &ReleaseRehearsal) -> String {
    let would_publish = rehearsal
        .verdicts
        .iter()
        .filter(|verdict| verdict.decision == PublishDecision::WouldPublish)
        .count();
    let tag_only = rehearsal
        .verdicts
        .iter()
        .filter(|verdict| verdict.decision == PublishDecision::TagOnly)
        .count();
    let already = rehearsal.verdicts.len() - would_publish - tag_only;
    let mut parts = vec![format!("{would_publish} would publish")];
    if tag_only > 0 {
        parts.push(format!("{tag_only} tag only"));
    }
    if already > 0 {
        parts.push(format!("{already} already published"));
    }
    format!("release (dry run): {}", parts.join(", "))
}

/// `release tag/publish`: drive the mutating release pipeline. `tag` stops
/// after commit/tag/push; `publish` continues to the registry. A bare
/// invocation (no `--yes`) previews the pending cut via [`confirm_or_preview`],
/// then fails closed on the missing confirmation.
pub(super) fn run(
    providers: &[&dyn Provider],
    supervisor: &Arc<ProcessSupervisor>,
    project: &Project,
    cli: &Cli,
    action: ReleaseAction,
) -> AppResult<ExitCode> {
    confirm_or_preview(providers, project, cli)?;
    let output = resolve_output(cli.execution.output, &project.document);
    let action_token = match action {
        ReleaseAction::Publish => "publish",
        _ => "tag",
    };
    print_release_header(action_token, output, cli.execution.color_choice());
    let request = release_request(project)?;
    let opened = project.open_member_vcs(providers, &BaselineFlags::new())?;
    let readers = opened.readers();
    let repos = opened.release_repos();
    let report = Report::resolve(
        cli.execution.output,
        cli.execution.verbosity(),
        cli.execution.color_choice(),
        &project.document,
    );
    let mut reporter = report.reporter();
    let sink: &mut dyn Reporter = reporter.as_mut();

    let overrides = build_overrides(cli)?;
    let options = ReleaseApplyOptions {
        no_push: cli.no_push,
        publish: matches!(action, ReleaseAction::Publish),
        ..ReleaseApplyOptions::default()
    };
    let hooks =
        crate::commands::hook::CliHookRunner::new(providers, supervisor, project, &cli.execution);
    let verb = match action {
        ReleaseAction::Publish => VerbId::Publish,
        _ => VerbId::Tag,
    };
    let runner: Arc<dyn ToolRunner> = Arc::new(ProcessToolRunner::new());
    let stats = release_run(
        &request,
        &project.document,
        providers,
        &readers,
        &repos,
        &overrides,
        sink,
        &hooks,
        verb,
        &options,
        &runner,
    )?;
    print_release_summary(
        &run_summary_line(action_token, stats.planned_modules),
        output,
        cli.execution.color_choice(),
    );
    Ok(release_exit(&run_outcome(&stats)))
}

pub(super) fn require_release_confirmation(confirmed: bool) -> AppResult<()> {
    if confirmed {
        return Ok(());
    }
    Err(AppError::invalid_input(
        "release.confirmation",
        "real releases require explicit confirmation; pass --yes to proceed",
    ))
}

/// Gate a mutating version-cut action (`bump` / `tag` / `publish`) on `--yes`,
/// but first show what it *would* cut. On a confirmed run this returns
/// immediately and the caller proceeds to mutate. On an unconfirmed run it
/// streams the same non-mutating `ModuleReleaseResolved` decision events as
/// `release plan` (honoring the per-run overrides and `--output`), then fails
/// closed with the confirmation error on stderr. A PLAN failure is the real
/// blocker and is surfaced instead of the confirmation error.
fn confirm_or_preview(providers: &[&dyn Provider], project: &Project, cli: &Cli) -> AppResult<()> {
    if cli.confirm_release {
        return Ok(());
    }
    let output = resolve_output(cli.execution.output, &project.document);
    let overrides = build_overrides(cli)?;
    print_release_header("plan", output, cli.execution.color_choice());
    let plan = stream_release_plan(providers, project, cli, &overrides)?;
    print_release_summary(
        &plan_summary_line(&plan),
        output,
        cli.execution.color_choice(),
    );
    require_release_confirmation(cli.confirm_release)
}

/// `release bump`: run only the version + changelog mutation phase, then stage
/// the mutation for a pull request. Never commits, tags, pushes, or publishes —
/// the commit/tag/push is `release tag` / `release publish` after the staged
/// change merges. `--dry-run` previews the mutation without writing, so it needs
/// no `--yes` confirmation; a bare invocation (no `--yes`, no `--dry-run`)
/// previews the pending cut via [`confirm_or_preview`], then fails closed on the
/// missing confirmation.
///
/// The project-level `[hooks.bump]` (composed with the umbrella `[hooks.release]`)
/// wrap a real bump: `pre` runs fail-closed before the mutation, `post` after it
/// stages successfully. A `--dry-run` preview mutates nothing, so it runs no
/// hooks.
pub(super) fn bump(
    providers: &[&dyn Provider],
    supervisor: &Arc<ProcessSupervisor>,
    project: &Project,
    cli: &Cli,
) -> AppResult<ExitCode> {
    if !cli.execution.dry_run {
        confirm_or_preview(providers, project, cli)?;
    }
    let request = release_request(project)?;
    let opened = project.open_member_vcs(providers, &BaselineFlags::new())?;
    let readers = opened.readers();
    let repos = opened.release_repos();
    let overrides = build_overrides(cli)?;
    let clock = crate::host::resolve_clock()?;
    let options = BumpOptions {
        dry_run: cli.execution.dry_run,
    };
    let lifecycle = if cli.execution.dry_run {
        toven_ports::HooksConfig::default()
    } else {
        project.document.hooks_for(VerbId::Bump)
    };
    let hook_runner =
        crate::commands::hook::CliHookRunner::new(providers, supervisor, project, &cli.execution);
    let report = Report::resolve(
        cli.execution.output,
        cli.execution.verbosity(),
        cli.execution.color_choice(),
        &project.document,
    );
    let mut reporter = report.reporter();
    let sink: &mut dyn Reporter = reporter.as_mut();
    let output = resolve_output(cli.execution.output, &project.document);
    crate::commands::hook::run_with_lifecycle(&lifecycle, &hook_runner, || {
        stream_bump(
            &request,
            &project.document,
            providers,
            &readers,
            &repos,
            &overrides,
            sink,
            clock.as_ref(),
            &hook_runner,
            options,
            output,
            cli.execution.color_choice(),
        )
    })
}

/// A stable JSON-lines record for one rehearsed publish verdict.
#[derive(Serialize)]
struct RehearsalRecord {
    kind: &'static str,
    module: String,
    publication: String,
    registry: Option<String>,
    version: String,
    decision: String,
}

/// A stable JSON-lines record for one rehearsed hosted forge Release.
#[derive(Serialize)]
struct HostRehearsalRecord {
    kind: &'static str,
    forge: String,
    tag: String,
    draft: bool,
    prerelease: bool,
    notes: String,
    assets: Vec<String>,
}

fn render_rehearsal_human(rehearsal: &ReleaseRehearsal) {
    let would_publish = rehearsal
        .verdicts
        .iter()
        .filter(|verdict| verdict.decision == PublishDecision::WouldPublish)
        .count();
    let tag_only = rehearsal
        .verdicts
        .iter()
        .filter(|verdict| verdict.decision == PublishDecision::TagOnly)
        .count();
    let already_published = rehearsal.verdicts.len() - would_publish - tag_only;
    let mut summary = OutputKV::new();
    summary
        .add("policy", rehearsal.policy.as_str().to_string())
        .add("would_publish", would_publish.to_string())
        .add("already_published", already_published.to_string())
        .add("tag_only", tag_only.to_string())
        .add("hosted_releases", rehearsal.hosted.len().to_string());
    println!("{summary}");
    let mut table = OutputTable::new(vec!["Module", "Publication", "Version", "Decision"])
        .with_title("Release rehearsal (no mutation)");
    for verdict in &rehearsal.verdicts {
        table.add_row(vec![
            verdict.module.to_string(),
            publication_label(&verdict.publication),
            verdict.version.to_string(),
            verdict.decision.as_str().to_string(),
        ]);
    }
    println!("{table}");
    if !rehearsal.hosted.is_empty() {
        let mut hosted =
            OutputTable::new(vec!["Forge", "Tag", "Draft", "Prerelease", "Asset count"])
                .with_title("Hosted releases (would cut)");
        for release in &rehearsal.hosted {
            hosted.add_row(vec![
                release.forge.clone(),
                release.tag.clone(),
                release.draft.to_string(),
                release.prerelease.to_string(),
                release.assets.len().to_string(),
            ]);
        }
        println!("{hosted}");
        for release in &rehearsal.hosted {
            if release.notes.is_empty() {
                continue;
            }
            println!("\nRelease notes — {} ({})", release.tag, release.forge);
            for line in release.notes.lines() {
                println!("  {line}");
            }
        }
    }
}

fn render_rehearsal_jsonl(rehearsal: &ReleaseRehearsal) -> AppResult<()> {
    for verdict in &rehearsal.verdicts {
        let record = RehearsalRecord {
            kind: "publish",
            module: verdict.module.to_string(),
            publication: verdict.publication.as_str().to_string(),
            registry: verdict.publication.registry().map(str::to_string),
            version: verdict.version.to_string(),
            decision: verdict.decision.as_str().to_string(),
        };
        let line = serde_json::to_string(&record).map_err(AppError::internal)?;
        println!("{line}");
    }
    for release in &rehearsal.hosted {
        let record = HostRehearsalRecord {
            kind: "hosted_release",
            forge: release.forge.clone(),
            tag: release.tag.clone(),
            draft: release.draft,
            prerelease: release.prerelease,
            notes: release.notes.clone(),
            assets: release.assets.clone(),
        };
        let line = serde_json::to_string(&record).map_err(AppError::internal)?;
        println!("{line}");
    }
    Ok(())
}
