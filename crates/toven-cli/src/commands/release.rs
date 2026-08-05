//! The `release` lifecycle verb: `plan`, `status`, `bump`, `tag`, `publish`.
//!
//! `plan` and `status` are read-only projections over the engine release spine
//! — they render typed data on stdout and never mutate a manifest, tag, or
//! registry. `bump` runs only the version + changelog mutation phase, then
//! commits the release (or, with `--no-commit`, stages it for a pull request)
//! without tagging, pushing, or publishing. `tag` and `publish` drive the
//! mutating release pipeline ([`release_run`]): `tag` stops after the release
//! commit/tag/push, `publish` continues to the registry. `publish` under
//! `--dry-run` instead runs a no-mutation rehearsal ([`release_rehearse`]) that
//! reports the resolved publish order and per-module
//! would-publish/already-published verdicts. Libraries return typed data; this
//! CLI layer is the only one that prints, following the introspection stream
//! convention (projection on stdout, warnings/summaries on stderr).

use rskit_cli::{ExitCode, OutputKV, OutputTable};
use rskit_errors::{AppError, AppResult};
use rskit_version::semver::Version;
use serde::Serialize;
use toven_engine::plan::PlanRequest;
use toven_engine::release::{
    BumpOptions, BumpOverrides, BumpReport, ChecksumReport, CosignSigner, CosignVerifier,
    DepgraphReport, GhAssetDownloader, PackageReport, ProcessVersionProbe, PublishDecision,
    ReadinessReport, ReleaseApplyOptions, ReleasePlan, ReleaseRehearsal, ReleaseStatus, SbomReport,
    SignReport, VerifyOptions, VerifyReport, release_bump, release_checksums, release_depgraphs,
    release_package, release_plan, release_readiness, release_rehearse, release_run, release_sbom,
    release_sign, release_status, release_verify,
};
use toven_engine::vcs::BaselineFlags;
use toven_model::{Event, ModuleRef};
use toven_ports::{BumpLevel, Provider, PublicationPolicy, Reporter, TaskIntent};

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
        ReleaseAction::Readiness => readiness(providers, project, cli.output),
        ReleaseAction::Sbom => sbom(providers, project, cli),
        ReleaseAction::Depgraphs => depgraphs(providers, project, cli),
        ReleaseAction::Package => package(providers, project, cli),
        ReleaseAction::Checksums => checksums(providers, project, cli),
        ReleaseAction::Sign => sign(providers, project, cli),
        ReleaseAction::Verify => verify(providers, project, cli),
        ReleaseAction::Publish if cli.dry_run => rehearse(providers, project, cli),
        ReleaseAction::Bump => bump(providers, project, cli),
        ReleaseAction::Tag | ReleaseAction::Publish => run(providers, project, cli, action),
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
        &BumpOverrides::new(),
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
    let out_dir = resolve_out_dir(cli, project);
    let mut reporter = QuietReporter;
    let report = release_sbom(
        &request,
        &project.document,
        providers,
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
    let target = cli.target.as_deref().ok_or_else(|| {
        AppError::invalid_input(
            "release.package.target",
            "`toven release package` requires `--target <triple>` (e.g. \
             x86_64-unknown-linux-gnu)",
        )
    })?;
    let mut reporter = QuietReporter;
    let report = release_package(
        &request,
        &project.document,
        providers,
        target,
        cli.binary.as_deref(),
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
    let mut reporter = QuietReporter;
    let report = release_checksums(&request, &project.document, providers, &mut reporter)?;
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
    let signer = CosignSigner::new();
    let mut reporter = QuietReporter;
    let report = release_sign(
        &request,
        &project.document,
        providers,
        &signer,
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
    let downloader = GhAssetDownloader::new();
    let verifier = CosignVerifier::new();
    let probe = ProcessVersionProbe::new();
    let options = VerifyOptions {
        download: cli.download,
        run: !cli.no_run,
    };
    let mut reporter = QuietReporter;
    let report = release_verify(
        &request,
        &project.document,
        providers,
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
/// after commit/tag/push; `publish` continues to the registry.
fn run(
    providers: &[&dyn Provider],
    project: &Project,
    cli: &Cli,
    action: ReleaseAction,
) -> AppResult<ExitCode> {
    require_release_confirmation(cli.confirm_release)?;
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
    let hooks = crate::commands::hook::CliHookRunner::new(providers, project, cli);
    release_run(
        &request,
        &project.document,
        providers,
        &readers,
        &repos,
        &overrides,
        sink,
        &hooks,
        &options,
    )?;
    Ok(ExitCode::Success)
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

/// `release bump`: run only the version + changelog mutation phase, then commit
/// the release (or, with `--no-commit`, stage it for a pull request). Never
/// tags, pushes, or publishes. `--dry-run` previews the mutation without
/// writing, so it needs no `--yes` confirmation.
fn bump(providers: &[&dyn Provider], project: &Project, cli: &Cli) -> AppResult<ExitCode> {
    if !cli.dry_run {
        require_release_confirmation(cli.confirm_release)?;
    }
    let request = release_request(project)?;
    let opened = project.open_member_vcs(providers, &BaselineFlags::new())?;
    let readers = opened.readers();
    let repos = opened.release_repos();
    let overrides = build_overrides(cli)?;
    let clock = crate::host::resolve_clock()?;
    let options = BumpOptions {
        no_commit: cli.no_commit,
        dry_run: cli.dry_run,
    };
    let mut reporter = QuietReporter;
    let report = release_bump(
        &request,
        &project.document,
        providers,
        &readers,
        &repos,
        &overrides,
        &mut reporter,
        clock.as_ref(),
        &options,
    )?;
    match resolve_output(cli.output, &project.document) {
        OutputKind::Jsonl => render_bump_jsonl(&report)?,
        OutputKind::Human => render_bump_human(&report),
    }
    Ok(ExitCode::Success)
}

/// A stable JSON-lines record for one `release bump` module outcome.
#[derive(Serialize)]
struct BumpRecord {
    module: String,
    old_version: String,
    new_version: String,
    manifests: Vec<String>,
    committed: bool,
    dry_run: bool,
    changelogs: Vec<String>,
}

/// Render the `release bump` report as one JSON-lines record per module.
fn render_bump_jsonl(report: &BumpReport) -> AppResult<()> {
    for module in &report.modules {
        let record = BumpRecord {
            module: module.module.to_string(),
            old_version: module.old_version.to_string(),
            new_version: module.new_version.to_string(),
            manifests: module.manifests.clone(),
            committed: report.committed,
            dry_run: report.dry_run,
            changelogs: report.changelogs.clone(),
        };
        let line = serde_json::to_string(&record).map_err(AppError::internal)?;
        println!("{line}");
    }
    Ok(())
}

/// Render the `release bump` report as a human table.
fn render_bump_human(report: &BumpReport) {
    if report.modules.is_empty() {
        println!("\nRelease bump — nothing to bump");
        println!("  all modules are up to date");
        return;
    }
    let disposition = if report.dry_run {
        "dry-run (nothing written)"
    } else if report.committed {
        "committed"
    } else {
        "staged (no commit)"
    };
    let title = format!("Release bump — {disposition}");
    let mut table = OutputTable::new(vec!["Module", "From", "To", "Manifests"]).with_title(title);
    for module in &report.modules {
        table.add_row(vec![
            module.module.to_string(),
            module.old_version.to_string(),
            module.new_version.to_string(),
            module.manifests.join(", "),
        ]);
    }
    println!("{table}");
    if !report.changelogs.is_empty() {
        let verb = if report.dry_run {
            "would roll"
        } else {
            "rolled"
        };
        println!("changelog {verb}: {}", report.changelogs.join(", "));
    }
}

/// A stable JSON-lines record for one `release plan` entry.
#[derive(Serialize)]
struct PlanRecord {
    /// 1-based position in the deterministic publication order.
    order: usize,
    module: String,
    current_version: String,
    planned_version: Option<String>,
    tag: Option<String>,
    level: Option<String>,
    reason: String,
    winning_input: String,
    cascade_origin: Option<String>,
    prerelease_channel: Option<String>,
    up_to_date: bool,
    publication: String,
    registry: Option<String>,
    publish_needed: bool,
    summary: String,
}

fn render_plan_human(plan: &ReleasePlan) {
    let title = format!("Release plan ({})", plan.policy.as_str());
    if plan.entries.is_empty() {
        println!("\n{title}");
        println!("  nothing to release: all modules are up to date");
        return;
    }
    let mut table = OutputTable::new(vec![
        "#",
        "Module",
        "Current",
        "Planned",
        "Tag",
        "Level",
        "Reason",
        "Input",
        "Publication",
        "Publish",
        "Summary",
    ])
    .with_title(title);
    for (index, entry) in plan.entries.iter().enumerate() {
        table.add_row(vec![
            (index + 1).to_string(),
            entry.module.to_string(),
            entry.current_version.to_string(),
            entry
                .planned_version
                .as_ref()
                .map_or_else(|| "-".to_string(), ToString::to_string),
            entry.planned_tag.clone().unwrap_or_else(|| "-".to_string()),
            if entry.planned_version.is_some() {
                entry.level.as_str().to_string()
            } else {
                "-".to_string()
            },
            entry.reason.as_str().to_string(),
            entry.winning_input.as_str().to_string(),
            publication_label(&entry.publication),
            if entry.up_to_date {
                "up to date".to_string()
            } else if entry.publish_needed {
                "yes".to_string()
            } else {
                "no".to_string()
            },
            entry.changelog.summary.clone(),
        ]);
    }
    println!("{table}");
}

fn render_plan_jsonl(plan: &ReleasePlan) -> AppResult<()> {
    for (index, entry) in plan.entries.iter().enumerate() {
        let record = PlanRecord {
            order: index + 1,
            module: entry.module.to_string(),
            current_version: entry.current_version.to_string(),
            planned_version: entry.planned_version.as_ref().map(ToString::to_string),
            tag: entry.planned_tag.clone(),
            level: entry
                .planned_version
                .as_ref()
                .map(|_| entry.level.as_str().to_string()),
            reason: entry.reason.as_str().to_string(),
            winning_input: entry.winning_input.as_str().to_string(),
            cascade_origin: entry.cascade_origin.as_ref().map(ToString::to_string),
            prerelease_channel: entry.prerelease_channel.clone(),
            up_to_date: entry.up_to_date,
            publication: entry.publication.as_str().to_string(),
            registry: entry.publication.registry().map(str::to_string),
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
    publication: String,
    registry: Option<String>,
    declared_version: String,
    latest_tag: Option<String>,
    host_forge: Option<String>,
    published_versions: Vec<String>,
    is_published: bool,
}

fn render_status_human(status: &ReleaseStatus) {
    let mut table = OutputTable::new(vec![
        "Module",
        "Publication",
        "Declared",
        "Latest tag",
        "Hosted on",
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
}

fn render_package_human(report: &PackageReport) {
    let mut table = OutputTable::new(vec!["Asset", "Source", "Format", "Bytes"])
        .with_title(format!("Release packages ({})", report.target));
    for asset in &report.assets {
        table.add_row(vec![
            asset.asset.clone(),
            asset.source.display().to_string(),
            asset.format.as_str().to_string(),
            asset.bytes.to_string(),
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
    signature: String,
    certificate: String,
}

fn render_sign_human(report: &SignReport) {
    let mut table = OutputTable::new(vec!["Blob", "Signature", "Certificate"])
        .with_title("Release signing".to_string());
    table.add_row(vec![
        report.blob.clone(),
        report.signature.clone(),
        report.certificate.clone(),
    ]);
    println!("{table}");
}

fn render_sign_jsonl(report: &SignReport) -> AppResult<()> {
    let record = SignRecord {
        blob: report.blob.clone(),
        signature: report.signature.clone(),
        certificate: report.certificate.clone(),
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

#[cfg(test)]
mod tests {
    use super::require_release_confirmation;

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
}
