//! Route a `toven release <action>` to its handler, plus the request and
//! override helpers every action shares.

use rskit_cli::{ExitCode, ShutdownController, ShutdownPolicy};
use rskit_errors::{AppError, AppResult};
use rskit_version::semver::Version;
use std::sync::Arc;
use toven_core::plan::PlanRequest;
use toven_exec::ProcessSupervisor;
use toven_model::ModuleRef;
use toven_ports::{BumpLevel, Provider, TaskIntent};
use toven_release::BumpOverrides;

use crate::flags::{Cli, ReleaseAction};
use crate::host::{Project, new_run_id};

use super::artifacts::{checksums, depgraphs, image, package, provenance, sbom, sign, verify};
use super::inspect::{readiness, status};
use super::lifecycle::{bump, plan, rehearse, run};

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
        ReleaseAction::Status => status(providers, supervisor, project, cli),
        ReleaseAction::Readiness => readiness(providers, supervisor, project, cli),
        ReleaseAction::Sbom => sbom(providers, supervisor, project, cli),
        ReleaseAction::Depgraphs => depgraphs(providers, supervisor, project, cli),
        ReleaseAction::Package => package(providers, supervisor, project, cli),
        ReleaseAction::Checksums => checksums(providers, supervisor, project, cli),
        ReleaseAction::Sign => sign(providers, supervisor, project, cli),
        ReleaseAction::Verify => verify(providers, supervisor, project, cli),
        ReleaseAction::Image => image(providers, supervisor, project, cli),
        ReleaseAction::Provenance => provenance(providers, supervisor, project, cli),
        ReleaseAction::Publish if cli.execution.dry_run => rehearse(providers, project, cli),
        ReleaseAction::Bump => bump(providers, supervisor, project, cli),
        ReleaseAction::Tag | ReleaseAction::Publish => {
            run(providers, supervisor, project, cli, action)
        }
    }
}

/// Build the validated per-run bump overrides from the parsed release argv.
pub(super) fn build_overrides(cli: &Cli) -> AppResult<BumpOverrides> {
    let mut overrides = BumpOverrides::new();
    for (modules, level) in [
        (&cli.patch, BumpLevel::Patch),
        (&cli.minor, BumpLevel::Minor),
        (&cli.major, BumpLevel::Major),
    ] {
        for module in modules {
            if module == WORKSPACE_SCOPE {
                overrides = overrides.with_workspace_level(level)?;
            } else {
                overrides = overrides.with_module_level(ModuleRef::parse(module)?, level)?;
            }
        }
    }
    for pair in &cli.set_version {
        match parse_set_version(pair)? {
            (Some(module), version) => {
                overrides = overrides.with_set_version(module, version)?;
            }
            // A bare `--set-version <version>` (no `<module>=`) targets every
            // in-scope module in one argv — the lock-step / ecosystem-wide cut.
            (None, version) => {
                overrides = overrides.with_workspace_set_version(version)?;
            }
        }
    }
    if let Some(channel) = &cli.pre {
        overrides = overrides.with_prerelease(channel.clone());
    }
    if let Some(base) = &cli.execution.base {
        overrides = overrides.with_base(base.clone());
    }
    Ok(overrides.with_offline(cli.offline))
}

/// The reserved `--patch`/`--minor`/`--major` value that selects the
/// repo-wide scope (every in-scope module, each from its own baseline)
/// instead of one module.
const WORKSPACE_SCOPE: &str = "*";

/// Parse a `--set-version` argument. `<module>=<x.y.z>` pins one module;
/// a bare `<x.y.z>` (no `=`) is the workspace-wide lock-step target.
fn parse_set_version(pair: &str) -> AppResult<(Option<ModuleRef>, Version)> {
    match pair.split_once('=') {
        Some((module, version)) => {
            let version = parse_target_version(version)?;
            Ok((Some(ModuleRef::parse(module)?), version))
        }
        None => Ok((None, parse_target_version(pair)?)),
    }
}

/// Parse a `--set-version` target version, surfacing an actionable error.
fn parse_target_version(version: &str) -> AppResult<Version> {
    Version::parse(version).map_err(|error| {
        AppError::invalid_input(
            "release.set-version",
            format!("invalid version '{version}': {error}"),
        )
    })
}

/// Build the release-scoped PLAN request rooted at the project.
pub(super) fn release_request(project: &Project) -> AppResult<PlanRequest> {
    Ok(PlanRequest::new(
        new_run_id()?,
        project.document.project.name.clone(),
        TaskIntent::resolve("release"),
        project.project_root.clone(),
    ))
}

/// The engine parallelism for a streamed release verb: `--jobs` /
/// `[toven].max_parallel` when set, else the machine's available parallelism
/// (mirroring `run`) so slow per-unit I/O overlaps.
pub(super) fn engine_jobs(cli: &Cli, project: &Project) -> usize {
    crate::commands::run::resolve_max_parallel(cli.execution.jobs, project)
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, std::num::NonZero::get))
}

/// Bridge a streamed release verb onto the async engine under graceful
/// shutdown.
///
/// Builds the Tokio runtime that bridges the sync CLI to the async engine,
/// installs the shared stop-signal controller *inside* that runtime (SIGINT /
/// SIGTERM / SIGHUP cancel a shared token; a second signal force-exits), and
/// threads that token into [`toven_runtime::execute`] — so on a signal the
/// engine stops launching waves and asks in-flight units to cancel instead of
/// running to completion and reporting success. Returns the aggregated summary
/// and whether the stop signal fired, so the caller can map an interrupted run
/// to [`ExitCode::Cancelled`] (`130`).
///
/// The `supervisor` is the shared process supervisor every release tool/registry
/// child registers with (the providers' runner and the CLI-local runners are all
/// built against it). Subscribing it to the same token as the backstop — exactly
/// as `run` does — means a fired signal terminates the *underlying supervised
/// process* of an in-flight blocking tool call (a slow `syft`/`cargo`/`cosign`
/// invocation), not merely the cooperative wrapper, so a long registry/tool call
/// is actually reaped rather than run to completion.
///
/// # Errors
/// Propagates runtime construction, shutdown-controller installation, backstop
/// subscription, and any hard operation error from the engine.
pub(super) fn drive_streamed<Op: toven_runtime::UnitOperation>(
    units: &[toven_runtime::UnitSpec],
    operation: Op,
    jobs: usize,
    supervisor: &Arc<ProcessSupervisor>,
    progress: &mut dyn toven_runtime::Progress<Op::Outcome>,
) -> AppResult<(toven_runtime::RunSummary, bool)> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(AppError::internal)?;
    runtime.block_on(async move {
        let shutdown = ShutdownController::install(ShutdownPolicy::default())?;
        let cancel = shutdown.token();
        // Backstop: on a stop signal the supervisor reaps every still-registered
        // child (provider- and CLI-runner-spawned alike), so an in-flight blocking
        // tool/registry call is terminated instead of run to completion.
        let _backstop = supervisor.subscribe_shutdown(cancel.clone())?;
        let summary = toven_runtime::execute(
            units,
            operation,
            toven_runtime::EngineConfig {
                jobs,
                fail_fast: false,
            },
            progress,
            cancel.clone(),
        )
        .await?;
        Ok((summary, cancel.is_cancelled()))
    })
}

/// The default artifact output directory when `--out-dir` is not given.
pub(super) fn resolve_out_dir(cli: &Cli, project: &Project) -> std::path::PathBuf {
    cli.out_dir.clone().unwrap_or_else(|| {
        project
            .project_root
            .as_path()
            .join("target")
            .join("toven")
            .join("release")
    })
}
