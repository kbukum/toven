//! The `release` lifecycle verb: `plan`, `status`, `bump`, `tag`, `publish`.
//!
//! `plan` and `status` are read-only projections over the engine release spine
//! — they render typed data on stdout and never mutate a manifest, tag, or
//! registry. `bump` runs only the version + changelog mutation phase, then
//! stages the mutation for a pull request without committing, tagging, pushing,
//! or publishing. `tag` and `publish` drive the
//! mutating release pipeline ([`release_run`]): `tag` stops after the release
//! commit/tag/push, `publish` continues to the registry. `publish` under
//! `--dry-run` instead runs a no-mutation rehearsal ([`release_rehearse`]) that
//! reports the resolved publish order and per-module
//! would-publish/already-published verdicts. Libraries return typed data; this
//! CLI layer is the only one that prints, following the introspection stream
//! convention (projection on stdout, warnings/summaries on stderr).

use rskit_cli::{ExitCode, OutputKV, OutputTable, Tone};
use rskit_errors::{AppError, AppResult};
use rskit_util::time::Clock;
use rskit_version::semver::Version;
use serde::Serialize;
use std::sync::Arc;
use toven_core::config::{Document, VerbId};
use toven_core::federation::MemberVcsReaders;
use toven_core::federation::member_repo::MemberReleaseRepos;
use toven_core::plan::PlanRequest;
use toven_core::vcs::BaselineFlags;
use toven_exec::{ProcessSupervisor, ProcessToolRunner};
use toven_model::{Entrypoint, ModuleRef, OutcomeSummary};
use toven_ports::{
    BumpLevel, HookRunner, Provider, PublicationPolicy, Reporter, TaskIntent, ToolRunner,
};
use toven_release::{
    BuildxImagePhase, BumpOptions, BumpOverrides, BumpReport, ChecksumReport, CosignSigner,
    CosignVerifier, DepgraphReport, GhAssetDownloader, GhAttestationProvenance, ImageOptions,
    ImageReport, PackageReport, ProcessVersionProbe, ProvenanceOptions, ProvenanceReport,
    PublishDecision, ReadinessReport, ReleaseApplyOptions, ReleasePlan, ReleaseRehearsal,
    ReleaseStats, ReleaseStatus, SbomReport, SignReport, VerifyOptions, VerifyReport, release_bump,
    release_checksums, release_depgraphs, release_image, release_package, release_plan,
    release_provenance, release_readiness, release_rehearse, release_run, release_sbom,
    release_sign, release_status, release_verify,
};

use crate::commands::support::QuietReporter;
use crate::flags::{Cli, ColorWhen, OutputKind, ReleaseAction};
use crate::host::{Project, Report, new_run_id, resolve_output};
use crate::report::stderr_theme;

/// Dispatch a `toven release <action>` invocation.
///
/// # Errors
/// Propagates release PLAN failures (configuration, discovery, graph) and, for
/// the mutating actions, APPLY failures (guardrails, mutation, tagging,
/// publishing).
pub(crate) fn execute(
    providers: &[&dyn Provider],
    supervisor: &Arc<ProcessSupervisor>,
    project: &Project,
    cli: &Cli,
    action: ReleaseAction,
) -> AppResult<ExitCode> {
    match action {
        ReleaseAction::Plan => plan(providers, project, cli),
        ReleaseAction::Status => status(providers, project, cli.output),
        ReleaseAction::Readiness => readiness(providers, project, cli.output),
        ReleaseAction::Sbom => sbom(providers, project, cli),
        ReleaseAction::Depgraphs => depgraphs(providers, project, cli),
        ReleaseAction::Package => package(providers, project, cli),
        ReleaseAction::Checksums => checksums(providers, project, cli),
        ReleaseAction::Sign => sign(providers, project, cli),
        ReleaseAction::Verify => verify(providers, project, cli),
        ReleaseAction::Image => image(providers, project, cli),
        ReleaseAction::Provenance => provenance(providers, project, cli),
        ReleaseAction::Publish if cli.dry_run => rehearse(providers, project, cli),
        ReleaseAction::Bump => bump(providers, supervisor, project, cli),
        ReleaseAction::Tag | ReleaseAction::Publish => {
            run(providers, supervisor, project, cli, action)
        }
    }
}

/// Build the validated per-run bump overrides from the parsed release argv.
fn build_overrides(cli: &Cli) -> AppResult<BumpOverrides> {
    let mut overrides = BumpOverrides::new();
    for (modules, level) in [
        (&cli.patch, BumpLevel::Patch),
        (&cli.minor, BumpLevel::Minor),
        (&cli.major, BumpLevel::Major),
    ] {
        for module in modules {
            overrides = overrides.with_module_level(ModuleRef::parse(module)?, level)?;
        }
    }
    for pair in &cli.set_version {
        let (module, version) = parse_set_version(pair)?;
        overrides = overrides.with_set_version(module, version)?;
    }
    if let Some(channel) = &cli.pre {
        overrides = overrides.with_prerelease(channel.clone());
    }
    if let Some(base) = &cli.base {
        overrides = overrides.with_base(base.clone());
    }
    Ok(overrides.with_offline(cli.offline))
}

/// Parse a `--set-version <module>=<x.y.z>` argument into its module and
/// target.
fn parse_set_version(pair: &str) -> AppResult<(ModuleRef, Version)> {
    let (module, version) = pair.split_once('=').ok_or_else(|| {
        AppError::invalid_input(
            "release.set-version",
            format!("expected '<module>=<x.y.z>', got '{pair}'"),
        )
    })?;
    let version = Version::parse(version).map_err(|error| {
        AppError::invalid_input(
            "release.set-version",
            format!("invalid version '{version}': {error}"),
        )
    })?;
    Ok((ModuleRef::parse(module)?, version))
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

/// `release plan`: project the release PLAN decision per module without
/// mutating anything — one live `ModuleReleaseResolved` per module (human on
/// stderr, JSONL on stdout), not a terminal table.
fn plan(providers: &[&dyn Provider], project: &Project, cli: &Cli) -> AppResult<ExitCode> {
    let output = resolve_output(cli.output, &project.document);
    print_release_header("plan", output, cli.color_choice());
    let plan = stream_release_plan(providers, project, cli, &BumpOverrides::new())?;
    print_release_summary(&plan_summary_line(&plan), output, cli.color_choice());
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
        cli.output,
        cli.verbosity(),
        cli.color_choice(),
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

/// Map a resolved release plan onto the shared item-based summary. A plan is a
/// read-only projection, so every entry is a `succeeded` item; a mutating
/// failure surfaces as an `Err`, never a `failed` summary count.
const fn plan_outcome(plan: &ReleasePlan) -> OutcomeSummary {
    let processed = plan.entries.len();
    OutcomeSummary {
        processed,
        succeeded: processed,
        failed: 0,
        skipped: 0,
    }
}

/// Map a completed `release bump` report onto the shared item-based summary.
/// Every reported module reached a good terminal state (a mid-transaction
/// failure restores the tree and returns an `Err`), so all are `succeeded`.
const fn bump_outcome(report: &BumpReport) -> OutcomeSummary {
    let processed = report.modules.len();
    OutcomeSummary {
        processed,
        succeeded: processed,
        failed: 0,
        skipped: 0,
    }
}

/// Map completed `release tag`/`publish` stats onto the shared item-based
/// summary. The transactional pipeline restores on any failure and returns an
/// `Err`, so a returned `ReleaseStats` counts only planned-and-completed
/// modules as `succeeded`.
const fn run_outcome(stats: &ReleaseStats) -> OutcomeSummary {
    let processed = stats.planned_modules;
    OutcomeSummary {
        processed,
        succeeded: processed,
        failed: 0,
        skipped: 0,
    }
}

/// Derive the process exit from the shared item-based summary — the single
/// owner of the failure verdict (step-01 [`OutcomeSummary`]), so release, task
/// runs, and coverage all map their exit through one path rather than a
/// hardcoded [`ExitCode::Success`].
const fn release_exit(summary: &OutcomeSummary) -> ExitCode {
    if summary.has_failures() {
        ExitCode::Failure
    } else {
        ExitCode::Success
    }
}

/// Print the release verb's header on stderr, human mode only.
///
/// A short title line rendered before the per-module decisions so they read as
/// a nested list; `--output jsonl` stays one record per module with no framing.
/// `action` is the canonical verb token (`plan`/`bump`/`tag`/`publish`).
fn print_release_header(action: &str, output: OutputKind, color: ColorWhen) {
    if matches!(output, OutputKind::Human) {
        eprintln!(
            "{}",
            stderr_theme(color).heading(&format!("Release {action}"))
        );
    }
}

/// Print a terminal release summary line on stderr, human mode only.
///
/// The closing aggregate that matches the summary-derived exit; suppressed
/// under `--output jsonl` so the machine stream stays events-only.
fn print_release_summary(line: &str, output: OutputKind, color: ColorWhen) {
    if matches!(output, OutputKind::Human) {
        eprintln!(
            "{}",
            stderr_theme(color).action("Finished", line, Tone::Success)
        );
    }
}

/// The truthful closing line for a `release plan` (or a mutating verb's
/// pre-confirmation preview), broken down by decision kind.
///
/// An empty plan states the up-to-date fact outright; a non-empty plan joins
/// only the non-zero groups (modules to release, dependency-floor-only moves,
/// already-released modules) so the reader scans exactly what will happen.
fn plan_summary_line(plan: &ReleasePlan) -> String {
    if plan.is_empty() {
        return "release: nothing to release — all modules up to date".to_string();
    }
    let (mut to_release, mut floor_only, mut up_to_date) = (0_usize, 0_usize, 0_usize);
    for entry in &plan.entries {
        if entry.up_to_date {
            up_to_date += 1;
        } else if entry.planned_version.is_some() {
            to_release += 1;
        } else {
            floor_only += 1;
        }
    }
    let mut parts = Vec::new();
    if to_release > 0 {
        parts.push(format!("{to_release} to release"));
    }
    if floor_only > 0 {
        parts.push(format!("{floor_only} dependency floor"));
    }
    if up_to_date > 0 {
        parts.push(format!("{up_to_date} up to date"));
    }
    format!("release: {}", parts.join(", "))
}

/// The truthful closing line for a mutating version-cut verb, phrased by the
/// side effect that actually landed (`staged`/`tagged`/`published`).
fn run_summary_line(action: &str, count: usize) -> String {
    let verb = match action {
        "bump" => "staged",
        "tag" => "tagged",
        "publish" => "published",
        _ => "released",
    };
    format!("release: {count} {verb}")
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
fn stream_bump(
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

/// The default artifact output directory when `--out-dir` is not given.
fn resolve_out_dir(cli: &Cli, project: &Project) -> std::path::PathBuf {
    cli.out_dir.clone().unwrap_or_else(|| {
        project
            .project_root
            .as_path()
            .join("target")
            .join("toven")
            .join("release")
    })
}

/// `release readiness`: evaluate the fail-closed go/no-go preflight, mutating
/// nothing. A no-go verdict exits non-zero so CI gates on it.
fn readiness(
    providers: &[&dyn Provider],
    project: &Project,
    output: Option<OutputKind>,
) -> AppResult<ExitCode> {
    let request = release_request(project)?;
    let opened = project.open_member_vcs(providers, &BaselineFlags::new())?;
    let readers = opened.readers();
    let mut reporter = QuietReporter;
    let report = release_readiness(
        &request,
        &project.document,
        providers,
        &readers,
        &mut reporter,
    )?;
    match resolve_output(output, &project.document) {
        OutputKind::Jsonl => render_readiness_jsonl(&report)?,
        OutputKind::Human => render_readiness_human(&report),
    }
    Ok(if report.is_go() {
        ExitCode::Success
    } else {
        ExitCode::Failure
    })
}

/// `release sbom`: generate a `CycloneDX` SBOM per releasable module under the
/// resolved output directory, mutating nothing outside it.
fn sbom(providers: &[&dyn Provider], project: &Project, cli: &Cli) -> AppResult<ExitCode> {
    let request = release_request(project)?;
    let opened = project.open_member_vcs(providers, &BaselineFlags::new())?;
    let readers = opened.readers();
    let out_dir = resolve_out_dir(cli, project);
    let mut reporter = QuietReporter;
    let report = release_sbom(
        &request,
        &project.document,
        providers,
        &readers,
        &out_dir,
        &mut reporter,
    )?;
    match resolve_output(cli.output, &project.document) {
        OutputKind::Jsonl => render_sbom_jsonl(&report)?,
        OutputKind::Human => render_sbom_human(&report),
    }
    Ok(ExitCode::Success)
}

/// `release depgraphs`: render the dependency graph to a DOT artifact under the
/// resolved output directory, mutating nothing outside it.
fn depgraphs(providers: &[&dyn Provider], project: &Project, cli: &Cli) -> AppResult<ExitCode> {
    let request = release_request(project)?;
    let out_dir = resolve_out_dir(cli, project);
    let mut reporter = QuietReporter;
    let report = release_depgraphs(
        &request,
        &project.document,
        providers,
        &out_dir,
        &mut reporter,
    )?;
    match resolve_output(cli.output, &project.document) {
        OutputKind::Jsonl => render_depgraphs_jsonl(&report)?,
        OutputKind::Human => render_depgraphs_human(&report),
    }
    Ok(ExitCode::Success)
}

/// `release package`: archive the already-built binary for `--target` into its
/// declared hosted-release asset, mutating no history. `--target` is required;
/// `--binary` overrides the default `target/<triple>/release/<binary>` source.
fn package(providers: &[&dyn Provider], project: &Project, cli: &Cli) -> AppResult<ExitCode> {
    let request = release_request(project)?;
    let opened = project.open_member_vcs(providers, &BaselineFlags::new())?;
    let readers = opened.readers();
    let target = cli.target.as_deref().ok_or_else(|| {
        AppError::invalid_input(
            "release.package.target",
            "`toven release package` requires `--target <triple>` (e.g. \
             x86_64-unknown-linux-gnu)",
        )
    })?;
    let mut reporter = QuietReporter;
    let tool_runner = ProcessToolRunner::new();
    let report = release_package(
        &request,
        &project.document,
        providers,
        &readers,
        target,
        cli.binary.as_deref(),
        &tool_runner,
        &mut reporter,
    )?;
    match resolve_output(cli.output, &project.document) {
        OutputKind::Jsonl => render_package_jsonl(&report)?,
        OutputKind::Human => render_package_human(&report),
    }
    Ok(ExitCode::Success)
}

/// `release checksums`: emit the `SHA256SUMS` manifest over the declared
/// release assets to its declared asset path, mutating no history.
fn checksums(providers: &[&dyn Provider], project: &Project, cli: &Cli) -> AppResult<ExitCode> {
    let request = release_request(project)?;
    let opened = project.open_member_vcs(providers, &BaselineFlags::new())?;
    let readers = opened.readers();
    let mut reporter = QuietReporter;
    let report = release_checksums(
        &request,
        &project.document,
        providers,
        &readers,
        &mut reporter,
    )?;
    match resolve_output(cli.output, &project.document) {
        OutputKind::Jsonl => render_checksums_jsonl(&report)?,
        OutputKind::Human => render_checksums_human(&report),
    }
    Ok(ExitCode::Success)
}

/// `release sign`: sign the declared `SHA256SUMS` manifest into its declared
/// detached-signature and certificate sidecars with cosign, mutating no history.
fn sign(providers: &[&dyn Provider], project: &Project, cli: &Cli) -> AppResult<ExitCode> {
    let request = release_request(project)?;
    let opened = project.open_member_vcs(providers, &BaselineFlags::new())?;
    let readers = opened.readers();
    let runner: Arc<dyn ToolRunner> = Arc::new(ProcessToolRunner::new());
    let signer = CosignSigner::new(runner.clone());
    let mut reporter = QuietReporter;
    let report = release_sign(
        &request,
        &project.document,
        providers,
        &readers,
        &signer,
        runner.as_ref(),
        &mut reporter,
    )?;
    match resolve_output(cli.output, &project.document) {
        OutputKind::Jsonl => render_sign_jsonl(&report)?,
        OutputKind::Human => render_sign_human(&report),
    }
    Ok(ExitCode::Success)
}

/// `release verify`: verify the declared release archives — locally (presence +
/// reported version) or, with `--download`, against the hosted release
/// (signature + checksum + reported version) — mutating nothing.
fn verify(providers: &[&dyn Provider], project: &Project, cli: &Cli) -> AppResult<ExitCode> {
    let request = release_request(project)?;
    let opened = project.open_member_vcs(providers, &BaselineFlags::new())?;
    let readers = opened.readers();
    let runner: Arc<dyn ToolRunner> = Arc::new(ProcessToolRunner::new());
    let downloader = GhAssetDownloader::new(runner.clone());
    let verifier = CosignVerifier::new(runner.clone());
    let probe = ProcessVersionProbe::new(runner.clone());
    let options = VerifyOptions {
        download: cli.download,
        run: !cli.no_run,
    };
    let mut reporter = QuietReporter;
    let report = release_verify(
        &request,
        &project.document,
        providers,
        &readers,
        options,
        &downloader,
        &verifier,
        &probe,
        &mut reporter,
    )?;
    match resolve_output(cli.output, &project.document) {
        OutputKind::Jsonl => render_verify_jsonl(&report)?,
        OutputKind::Human => render_verify_human(&report),
    }
    Ok(ExitCode::Success)
}

/// `release image`: build the configured container image once, push it to the
/// primary registry plus mirrors immutably, and cosign-sign the pushed digest.
/// `--dry-run` previews the references and existing digests mutation-free, so it
/// needs no `--yes`; the real push requires confirmation.
fn image(providers: &[&dyn Provider], project: &Project, cli: &Cli) -> AppResult<ExitCode> {
    if !cli.dry_run {
        require_release_confirmation(cli.confirm_release)?;
    }
    let request = release_request(project)?;
    let opened = project.open_member_vcs(providers, &BaselineFlags::new())?;
    let readers = opened.readers();
    let runner: Arc<dyn ToolRunner> = Arc::new(ProcessToolRunner::new());
    let image_phase = BuildxImagePhase::new(runner.clone());
    let options = ImageOptions {
        dry_run: cli.dry_run,
    };
    let mut reporter = QuietReporter;
    let report = release_image(
        &request,
        &project.document,
        providers,
        &readers,
        &image_phase,
        options,
        &mut reporter,
    )?;
    match resolve_output(cli.output, &project.document) {
        OutputKind::Jsonl => render_image_jsonl(&report)?,
        OutputKind::Human => render_image_human(&report),
    }
    Ok(ExitCode::Success)
}

/// `release provenance`: verify that exactly the published subjects (the
/// declared `SHA256SUMS` entries plus pushed image digests) carry a
/// build-provenance attestation cut by the CI trusted builder. `--dry-run`
/// reports presence without failing; the default run fails closed if any
/// subject lacks an attestation. Read-only, so it needs no `--yes`.
fn provenance(providers: &[&dyn Provider], project: &Project, cli: &Cli) -> AppResult<ExitCode> {
    let request = release_request(project)?;
    let opened = project.open_member_vcs(providers, &BaselineFlags::new())?;
    let readers = opened.readers();
    let runner: Arc<dyn ToolRunner> = Arc::new(ProcessToolRunner::new());
    let provenance_phase = GhAttestationProvenance::new(runner.clone());
    let image_phase = BuildxImagePhase::new(runner.clone());
    let options = ProvenanceOptions {
        dry_run: cli.dry_run,
    };
    let mut reporter = QuietReporter;
    let report = release_provenance(
        &request,
        &project.document,
        providers,
        &readers,
        &provenance_phase,
        &image_phase,
        options,
        &mut reporter,
    )?;
    match resolve_output(cli.output, &project.document) {
        OutputKind::Jsonl => render_provenance_jsonl(&report)?,
        OutputKind::Human => render_provenance_human(&report),
    }
    Ok(ExitCode::Success)
}

/// `release publish --dry-run`: rehearse the publish loop, mutating nothing.
fn rehearse(providers: &[&dyn Provider], project: &Project, cli: &Cli) -> AppResult<ExitCode> {
    let request = release_request(project)?;
    let opened = project.open_member_vcs(providers, &BaselineFlags::new())?;
    let readers = opened.readers();
    let overrides = build_overrides(cli)?;
    let mut reporter = QuietReporter;
    let rehearsal = release_rehearse(
        &request,
        &project.document,
        providers,
        &readers,
        &overrides,
        &mut reporter,
        cli.no_push,
    )?;
    match resolve_output(cli.output, &project.document) {
        OutputKind::Jsonl => render_rehearsal_jsonl(&rehearsal)?,
        OutputKind::Human => render_rehearsal_human(&rehearsal),
    }
    Ok(ExitCode::Success)
}

/// `release tag/publish`: drive the mutating release pipeline. `tag` stops
/// after commit/tag/push; `publish` continues to the registry. A bare
/// invocation (no `--yes`) previews the pending cut via [`confirm_or_preview`],
/// then fails closed on the missing confirmation.
fn run(
    providers: &[&dyn Provider],
    supervisor: &Arc<ProcessSupervisor>,
    project: &Project,
    cli: &Cli,
    action: ReleaseAction,
) -> AppResult<ExitCode> {
    confirm_or_preview(providers, project, cli)?;
    let output = resolve_output(cli.output, &project.document);
    let action_token = match action {
        ReleaseAction::Publish => "publish",
        _ => "tag",
    };
    print_release_header(action_token, output, cli.color_choice());
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

    let overrides = build_overrides(cli)?;
    let options = ReleaseApplyOptions {
        no_push: cli.no_push,
        publish: matches!(action, ReleaseAction::Publish),
        ..ReleaseApplyOptions::default()
    };
    let hooks = crate::commands::hook::CliHookRunner::new(providers, supervisor, project, cli);
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
        cli.color_choice(),
    );
    Ok(release_exit(&run_outcome(&stats)))
}

fn require_release_confirmation(confirmed: bool) -> AppResult<()> {
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
    let output = resolve_output(cli.output, &project.document);
    let overrides = build_overrides(cli)?;
    print_release_header("plan", output, cli.color_choice());
    let plan = stream_release_plan(providers, project, cli, &overrides)?;
    print_release_summary(&plan_summary_line(&plan), output, cli.color_choice());
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
fn bump(
    providers: &[&dyn Provider],
    supervisor: &Arc<ProcessSupervisor>,
    project: &Project,
    cli: &Cli,
) -> AppResult<ExitCode> {
    if !cli.dry_run {
        confirm_or_preview(providers, project, cli)?;
    }
    let request = release_request(project)?;
    let opened = project.open_member_vcs(providers, &BaselineFlags::new())?;
    let readers = opened.readers();
    let repos = opened.release_repos();
    let overrides = build_overrides(cli)?;
    let clock = crate::host::resolve_clock()?;
    let options = BumpOptions {
        dry_run: cli.dry_run,
    };
    let lifecycle = if cli.dry_run {
        toven_ports::HooksConfig::default()
    } else {
        project.document.hooks_for(VerbId::Bump)
    };
    let hook_runner =
        crate::commands::hook::CliHookRunner::new(providers, supervisor, project, cli);
    let report = Report::resolve(
        cli.output,
        cli.verbosity(),
        cli.color_choice(),
        &project.document,
    );
    let mut reporter = report.reporter();
    let sink: &mut dyn Reporter = reporter.as_mut();
    let output = resolve_output(cli.output, &project.document);
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
            cli.color_choice(),
        )
    })
}

/// A stable JSON-lines record for one `release status` module.
#[derive(Serialize)]
struct StatusRecord {
    module: String,
    publication: String,
    registry: Option<String>,
    declared_version: String,
    latest_tag: Option<String>,
    host_forge: Option<String>,
    published_versions: Vec<String>,
    is_published: bool,
    entrypoint: String,
    maintainer_tag_present: Option<bool>,
}

fn render_status_human(status: &ReleaseStatus) {
    let mut table = OutputTable::new(vec![
        "Module",
        "Publication",
        "Declared",
        "Latest tag",
        "Hosted on",
        "Flow",
        "Published",
    ])
    .with_title("Release status");
    for module in &status.modules {
        table.add_row(vec![
            module.module.to_string(),
            publication_label(&module.publication),
            module.declared_version.to_string(),
            module.latest_tag.clone().unwrap_or_else(|| "-".to_string()),
            module.host_forge.clone().unwrap_or_else(|| "-".to_string()),
            status_flow_label(module.entrypoint, module.maintainer_tag_present),
            if module.is_published { "yes" } else { "no" }.to_string(),
        ]);
    }
    println!("{table}");
}

fn render_status_jsonl(status: &ReleaseStatus) -> AppResult<()> {
    for module in &status.modules {
        let record = StatusRecord {
            module: module.module.to_string(),
            publication: module.publication.as_str().to_string(),
            registry: module.publication.registry().map(str::to_string),
            declared_version: module.declared_version.to_string(),
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
        };
        let line = serde_json::to_string(&record).map_err(AppError::internal)?;
        println!("{line}");
    }
    Ok(())
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

fn publication_label(publication: &PublicationPolicy) -> String {
    publication.registry().map_or_else(
        || publication.as_str().to_string(),
        |registry| format!("{} ({registry})", publication.as_str()),
    )
}

/// Human label for a module's release flow in `release status`. For a
/// maintainer-owned module it surfaces whether the maintainer's release tag for
/// the declared version is already present — a fail-closed readiness signal
/// (`tag missing` means Toven cannot yet publish against it).
fn status_flow_label(entrypoint: Entrypoint, maintainer_tag_present: Option<bool>) -> String {
    match maintainer_tag_present {
        Some(true) => format!("{} (tag ready)", entrypoint.as_str()),
        Some(false) => format!("{} (tag missing)", entrypoint.as_str()),
        None => entrypoint.as_str().to_string(),
    }
}

/// A stable JSON-lines record for one readiness check.
#[derive(Serialize)]
struct ReadinessRecord {
    check: String,
    passed: bool,
    detail: String,
}

fn render_readiness_human(report: &ReadinessReport) {
    if report.checks.is_empty() {
        // An empty-bordered table reads as broken output and a "go" with zero
        // rows is false confidence; state the absence explicitly instead.
        println!("\nRelease readiness");
        println!("  no readiness checks configured");
    } else {
        let mut table =
            OutputTable::new(vec!["Check", "Result", "Detail"]).with_title("Release readiness");
        for check in &report.checks {
            table.add_row(vec![
                check.name.clone(),
                if check.passed { "pass" } else { "fail" }.to_string(),
                check.detail.clone(),
            ]);
        }
        println!("{table}");
    }
    // Verdict last: the go/no-go conclusion reads after the evidence it summarizes.
    let mut verdict = OutputKV::new();
    verdict.add(
        "verdict",
        if report.is_go() { "go" } else { "no-go" }.to_string(),
    );
    print!("{verdict}");
}

fn render_readiness_jsonl(report: &ReadinessReport) -> AppResult<()> {
    for check in &report.checks {
        let record = ReadinessRecord {
            check: check.name.clone(),
            passed: check.passed,
            detail: check.detail.clone(),
        };
        let line = serde_json::to_string(&record).map_err(AppError::internal)?;
        println!("{line}");
    }
    Ok(())
}

/// A stable JSON-lines record for one generated SBOM artifact.
#[derive(Serialize)]
struct SbomRecord {
    module: String,
    path: String,
}

/// A stable JSON-lines record for one staged SBOM release asset.
#[derive(Serialize)]
struct StagedSbomRecord {
    asset: String,
    source: String,
}

fn render_sbom_human(report: &SbomReport) {
    let mut table =
        OutputTable::new(vec!["Module", "Artifact"]).with_title("Release SBOM artifacts");
    for artifact in &report.artifacts {
        table.add_row(vec![
            artifact.label.clone(),
            artifact.path.display().to_string(),
        ]);
    }
    println!("{table}");
    if !report.staged.is_empty() {
        let mut staged = OutputTable::new(vec!["Asset", "Source"]).with_title("Staged SBOM assets");
        for asset in &report.staged {
            staged.add_row(vec![asset.asset.clone(), asset.source.clone()]);
        }
        println!("{staged}");
    }
    for module in &report.skipped {
        eprintln!("warning: {module} skipped (ecosystem has no SBOM tooling)");
    }
}

fn render_sbom_jsonl(report: &SbomReport) -> AppResult<()> {
    for artifact in &report.artifacts {
        let record = SbomRecord {
            module: artifact.label.clone(),
            path: artifact.path.display().to_string(),
        };
        let line = serde_json::to_string(&record).map_err(AppError::internal)?;
        println!("{line}");
    }
    for asset in &report.staged {
        let record = StagedSbomRecord {
            asset: asset.asset.clone(),
            source: asset.source.clone(),
        };
        let line = serde_json::to_string(&record).map_err(AppError::internal)?;
        println!("{line}");
    }
    for module in &report.skipped {
        eprintln!("warning: {module} skipped (ecosystem has no SBOM tooling)");
    }
    Ok(())
}

/// A stable JSON-lines record for one generated dependency-graph artifact.
#[derive(Serialize)]
struct DepgraphRecord {
    label: String,
    path: String,
}

fn render_depgraphs_human(report: &DepgraphReport) {
    let mut table =
        OutputTable::new(vec!["Graph", "Artifact"]).with_title("Release dependency graphs");
    for artifact in &report.artifacts {
        table.add_row(vec![
            artifact.label.clone(),
            artifact.path.display().to_string(),
        ]);
    }
    println!("{table}");
}

fn render_depgraphs_jsonl(report: &DepgraphReport) -> AppResult<()> {
    for artifact in &report.artifacts {
        let record = DepgraphRecord {
            label: artifact.label.clone(),
            path: artifact.path.display().to_string(),
        };
        let line = serde_json::to_string(&record).map_err(AppError::internal)?;
        println!("{line}");
    }
    Ok(())
}

/// A stable JSON-lines record for one packaged release archive.
#[derive(Serialize)]
struct PackageRecord {
    target: String,
    asset: String,
    source: String,
    format: String,
    bytes: u64,
    backing: String,
}

fn render_package_human(report: &PackageReport) {
    let mut table = OutputTable::new(vec!["Asset", "Source", "Format", "Bytes", "Backing"])
        .with_title(format!("Release packages ({})", report.target));
    for asset in &report.assets {
        table.add_row(vec![
            asset.asset.clone(),
            asset.source.display().to_string(),
            asset.format.as_str().to_string(),
            asset.bytes.to_string(),
            asset.backing.to_string(),
        ]);
    }
    println!("{table}");
}

fn render_package_jsonl(report: &PackageReport) -> AppResult<()> {
    for asset in &report.assets {
        let record = PackageRecord {
            target: report.target.clone(),
            asset: asset.asset.clone(),
            source: asset.source.display().to_string(),
            format: asset.format.as_str().to_string(),
            bytes: asset.bytes,
            backing: asset.backing.to_string(),
        };
        let line = serde_json::to_string(&record).map_err(AppError::internal)?;
        println!("{line}");
    }
    Ok(())
}

/// A stable JSON-lines record for one checksummed release asset.
#[derive(Serialize)]
struct ChecksumRecord {
    manifest: String,
    name: String,
    sha256: String,
    bytes: u64,
}

fn render_checksums_human(report: &ChecksumReport) {
    let mut table = OutputTable::new(vec!["Asset", "SHA-256", "Bytes"])
        .with_title(format!("Release checksums ({})", report.manifest));
    for entry in &report.entries {
        table.add_row(vec![
            entry.name.clone(),
            entry.sha256.clone(),
            entry.bytes.to_string(),
        ]);
    }
    println!("{table}");
}

fn render_checksums_jsonl(report: &ChecksumReport) -> AppResult<()> {
    for entry in &report.entries {
        let record = ChecksumRecord {
            manifest: report.manifest.clone(),
            name: entry.name.clone(),
            sha256: entry.sha256.clone(),
            bytes: entry.bytes,
        };
        let line = serde_json::to_string(&record).map_err(AppError::internal)?;
        println!("{line}");
    }
    Ok(())
}

/// A stable JSON-lines record for the signing outputs.
#[derive(Serialize)]
struct SignRecord {
    blob: String,
    bundle: String,
    backing: String,
}

fn render_sign_human(report: &SignReport) {
    let mut table = OutputTable::new(vec!["Blob", "Bundle", "Backing"])
        .with_title("Release signing".to_string());
    table.add_row(vec![
        report.blob.clone(),
        report.bundle.clone(),
        report.backing.to_string(),
    ]);
    println!("{table}");
}

fn render_sign_jsonl(report: &SignReport) -> AppResult<()> {
    let record = SignRecord {
        blob: report.blob.clone(),
        bundle: report.bundle.clone(),
        backing: report.backing.to_string(),
    };
    let line = serde_json::to_string(&record).map_err(AppError::internal)?;
    println!("{line}");
    Ok(())
}

/// A stable JSON-lines record for one archive's verification outcome.
#[derive(Serialize)]
struct VerifiedAssetRecord {
    mode: String,
    tag: Option<String>,
    expected_version: String,
    name: String,
    checksum_ok: Option<bool>,
    signature_ok: Option<bool>,
    ran: bool,
    reported_version: Option<String>,
}

fn render_verify_human(report: &VerifyReport) {
    let mut table = OutputTable::new(vec!["Asset", "Checksum", "Signature", "Ran", "Reported"])
        .with_title(format!(
            "Release verify ({}, expected {})",
            report.mode.as_str(),
            report.expected_version
        ));
    for asset in &report.assets {
        table.add_row(vec![
            asset.name.clone(),
            render_optional_check(asset.checksum_ok),
            render_optional_check(asset.signature_ok),
            if asset.ran { "yes" } else { "no" }.to_string(),
            asset
                .reported_version
                .clone()
                .unwrap_or_else(|| "—".to_string()),
        ]);
    }
    println!("{table}");
}

/// Render an optional pass/fail check for the human table.
fn render_optional_check(value: Option<bool>) -> String {
    match value {
        Some(true) => "ok".to_string(),
        Some(false) => "FAIL".to_string(),
        None => "—".to_string(),
    }
}

fn render_verify_jsonl(report: &VerifyReport) -> AppResult<()> {
    for asset in &report.assets {
        let record = VerifiedAssetRecord {
            mode: report.mode.as_str().to_string(),
            tag: report.tag.clone(),
            expected_version: report.expected_version.clone(),
            name: asset.name.clone(),
            checksum_ok: asset.checksum_ok,
            signature_ok: asset.signature_ok,
            ran: asset.ran,
            reported_version: asset.reported_version.clone(),
        };
        let line = serde_json::to_string(&record).map_err(AppError::internal)?;
        println!("{line}");
    }
    Ok(())
}

/// A stable JSON-lines record for one module's `release image` outcome.
#[derive(Serialize)]
struct ImageRecord {
    preview: bool,
    module: String,
    references: Vec<String>,
    digest: Option<String>,
    signed: bool,
    status: String,
}

fn render_image_human(report: &ImageReport) {
    let title = if report.preview {
        "Release image (preview)".to_string()
    } else {
        "Release image".to_string()
    };
    let mut table = OutputTable::new(vec!["Module", "Reference", "Digest", "Signed", "Status"])
        .with_title(title);
    for outcome in &report.images {
        table.add_row(vec![
            outcome.module.clone(),
            outcome.references.join(", "),
            outcome.digest.clone().unwrap_or_else(|| "—".to_string()),
            if outcome.signed { "yes" } else { "no" }.to_string(),
            outcome.status.as_str().to_string(),
        ]);
    }
    println!("{table}");
}

fn render_image_jsonl(report: &ImageReport) -> AppResult<()> {
    for outcome in &report.images {
        let record = ImageRecord {
            preview: report.preview,
            module: outcome.module.clone(),
            references: outcome.references.clone(),
            digest: outcome.digest.clone(),
            signed: outcome.signed,
            status: outcome.status.as_str().to_string(),
        };
        let line = serde_json::to_string(&record).map_err(AppError::internal)?;
        println!("{line}");
    }
    Ok(())
}

/// A stable JSON-lines record for one `release provenance` subject.
#[derive(Serialize)]
struct ProvenanceRecord {
    preview: bool,
    status: String,
    name: String,
    digest: String,
}

fn render_provenance_human(report: &ProvenanceReport) {
    let title = if report.preview {
        format!("Release provenance (preview, {})", report.status.as_str())
    } else {
        format!("Release provenance ({})", report.status.as_str())
    };
    let mut table = OutputTable::new(vec!["Subject", "Digest", "Status"]).with_title(title);
    for entry in &report.subjects {
        table.add_row(vec![
            entry.subject.name.clone(),
            entry.subject.digest.clone(),
            entry.status.as_str().to_string(),
        ]);
    }
    println!("{table}");
}

fn render_provenance_jsonl(report: &ProvenanceReport) -> AppResult<()> {
    for entry in &report.subjects {
        let record = ProvenanceRecord {
            preview: report.preview,
            status: entry.status.as_str().to_string(),
            name: entry.subject.name.clone(),
            digest: entry.subject.digest.clone(),
        };
        let line = serde_json::to_string(&record).map_err(AppError::internal)?;
        println!("{line}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use rskit_cli::ExitCode;
    use rskit_util::time::FixedClock;
    use toven_core::config::{CanonicalRegistry, Document, load};
    use toven_core::federation::MemberVcsReaders;
    use toven_core::federation::baseline::MemberVcsReader;
    use toven_core::federation::member_repo::{MemberReleaseRepo, MemberReleaseRepos};
    use toven_core::plan::PlanRequest;
    use toven_model::{
        AbsPath, EcosystemId, Event, Module, ModuleRef, OutcomeSummary, RepoPath, ToolchainTag,
        Workspace, WorkspaceId,
    };
    use toven_ports::{DiscoverResponse, Provider, TaskIntent};
    use toven_release::{BumpOptions, BumpOverrides};
    use toven_testkit::workspace::workspace;
    use toven_testkit::{
        FakeConfiguredAdapter, FakeProvider, FakeReleaseTarget, FakeVcsReader, FakeVcsWriter,
        RecordingHookRunner, RecordingReporter, TestWorkspace,
    };

    use super::{release_exit, require_release_confirmation, stream_bump};

    #[test]
    fn empty_release_plan_summary_states_the_up_to_date_fact() {
        use toven_release::{BumpPolicy, ReleasePlan};

        use super::plan_summary_line;

        // An up-to-date workspace resolves an empty plan: the streamed decision
        // events emit nothing, so the closing summary must state the up-to-date
        // fact outright rather than leave a silent stream.
        let empty = ReleasePlan::new(BumpPolicy::SemverCascade, Vec::new());
        assert_eq!(
            plan_summary_line(&empty),
            "release: nothing to release — all modules up to date",
        );
    }

    #[test]
    fn run_summary_line_phrases_by_the_landed_side_effect() {
        use super::run_summary_line;

        assert_eq!(run_summary_line("bump", 2), "release: 2 staged");
        assert_eq!(run_summary_line("tag", 3), "release: 3 tagged");
        assert_eq!(run_summary_line("publish", 1), "release: 1 published");
    }

    #[test]
    fn real_release_requires_explicit_confirmation() {
        let error = require_release_confirmation(false).expect_err("confirmation is required");
        assert!(error.to_string().contains("release.confirmation"));
        assert!(error.to_string().contains("--yes"));
    }

    #[test]
    fn explicit_confirmation_allows_real_release_to_continue() {
        require_release_confirmation(true).expect("confirmation permits release");
    }

    #[test]
    fn release_exit_is_derived_from_the_item_summary() {
        // The exit owner is the item-based summary, never a hardcoded success:
        // an all-succeeded run exits zero, any failed item exits non-zero.
        let clean = OutcomeSummary {
            processed: 3,
            succeeded: 3,
            failed: 0,
            skipped: 0,
        };
        assert_eq!(release_exit(&clean), ExitCode::Success);

        let failed = OutcomeSummary {
            processed: 3,
            succeeded: 2,
            failed: 1,
            skipped: 0,
        };
        assert_eq!(release_exit(&failed), ExitCode::Failure);
    }

    /// A single-`core`-module rust provider rooted at the repo, releasable
    /// through the default fake release target (declares a version, rewrites a
    /// manifest — enough for a `bump` to stage a mutation).
    fn core_provider() -> FakeProvider {
        let eid = EcosystemId::new("rust").expect("ecosystem id");
        let mut response = DiscoverResponse::new(eid.clone());
        response.workspaces.push(Workspace::new(
            WorkspaceId::new("rust").expect("workspace id"),
            RepoPath::new(".").expect("root"),
            ToolchainTag::new("cargo"),
        ));
        let mut module = Module::new(
            ModuleRef::new(eid.clone(), "core").expect("module ref"),
            RepoPath::new(".").expect("root"),
        );
        module.workspace = Some(WorkspaceId::new("rust").expect("workspace id"));
        response.modules.push(module);
        let adapter = FakeConfiguredAdapter::new(eid.clone())
            .with_response(response)
            .with_release_target(FakeReleaseTarget::new());
        FakeProvider::new(eid).with_adapter(adapter)
    }

    /// Load a single-repo project whose `core` module stages a version bump.
    fn single_module_project() -> (TestWorkspace, AbsPath, Document) {
        let ws = workspace("release-cli-bump");
        let body = "[project]\nname = \"solo\"\n\n[ecosystems.rust]\nmanifests = [\"Cargo.toml\"]\n\n[modules.\"rust:core\".release]\npush = false\ncommit_message = \"release {module} {version}\"\n";
        let path = ws
            .write_file("toven.toml", body.as_bytes())
            .expect("write project");
        let root = AbsPath::new(ws.path().to_path_buf()).expect("absolute root");
        let document = load(&path, &BTreeSet::new(), &CanonicalRegistry::model())
            .expect("project loads")
            .document;
        (ws, root, document)
    }

    #[test]
    fn stream_bump_feeds_the_live_sink_decision_then_commit() {
        // The CLI seam must route the engine's per-module events into the live
        // reporter — the decision (resolved) up front, then the commit (staged)
        // once the mutation lands — and derive the exit from the run summary.
        let (ws, root, document) = single_module_project();
        let provider = core_provider();
        let providers: Vec<&dyn Provider> = vec![&provider];
        let reader = FakeVcsReader::new();
        let writer = FakeVcsWriter::new().with_commit_oid("unused");
        let readers = MemberVcsReaders::new(vec![MemberVcsReader::new(None, ".", None, &reader)]);
        let repos = MemberReleaseRepos::new(vec![MemberReleaseRepo::new(
            None,
            ws.path().to_path_buf(),
            &reader,
            &writer,
        )]);
        let request = PlanRequest::new("bump-1", "solo", TaskIntent::resolve("release"), root);
        let clock = FixedClock::new(1_718_409_600, 0);
        let hooks = RecordingHookRunner::new();
        let mut sink = RecordingReporter::new();

        let exit = stream_bump(
            &request,
            &document,
            &providers,
            &readers,
            &repos,
            &BumpOverrides::new(),
            &mut sink,
            &clock,
            &hooks,
            BumpOptions::default(),
            crate::flags::OutputKind::Jsonl,
            crate::flags::ColorWhen::Never,
        )
        .expect("bump streams");

        // The summary-derived exit for a clean bump is success.
        assert_eq!(exit, ExitCode::Success);

        let resolved = sink
            .events()
            .iter()
            .position(|event| matches!(event, Event::ModuleReleaseResolved { .. }))
            .expect("a decision event is streamed");
        let staged = sink
            .events()
            .iter()
            .position(|event| matches!(event, Event::ModuleReleaseStaged { .. }))
            .expect("a commit event is streamed");
        assert!(
            resolved < staged,
            "the decision must precede the commit: {:?}",
            sink.events()
        );
    }
}
