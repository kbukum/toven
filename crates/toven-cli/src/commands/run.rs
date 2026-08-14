//! Execution verbs: the argv-first task and the `run <task>` escape hatch.
//!
//! These are the verbs with an APPLY half. Each builds the typed
//! [`PlanRequest`], binds the rskit-backed engine ports, emits the CLI-owned
//! [`Event::RunStarted`], and calls the engine PLAN spine. A `--dry-run` /
//! `--explain` cut stops at PLAN and synthesizes the terminal summary from the
//! immutable [`Plan`]; a full run drives APPLY on a Tokio runtime with
//! caller-declared graceful shutdown (SIGINT/SIGTERM/SIGHUP → cooperative
//! teardown, backed by the runner's process supervisor). Shutdown is installed
//! at the composition root before PLAN, and one shared [`ProcessSupervisor`] is
//! injected into both the toolchain prober's tool runner and the APPLY command
//! runner, so a stop signal during a toolchain probe cancels and reaps rather
//! than orphaning the probe child under the OS default action. The `release`
//! lifecycle lives in its own
//! [`commands::release`](crate::commands::release) module.

use std::sync::Arc;

use rskit_cli::{ExitCode, ShutdownController, ShutdownPolicy};
use rskit_errors::{AppError, AppResult};
use rskit_process::{LifecyclePolicy, ProcessSupervisor};
use toven_core::config::ViewMode;
use toven_core::federation::MemberVcsReaders;
use toven_core::plan::{CacheMode, PlanHost, PlanRequest, plan};
use toven_engine::apply::{ApplyOptions, apply};
use toven_engine::cache::FsContentCache;
use toven_engine::source::FsSourceDigest;
use toven_engine::toolchain::ProcessToolchainProber;
use toven_exec::ProcessToolRunner;
use toven_model::{CacheVerdict, Event, Plan, RunStats};
use toven_ports::{PlanReporter, Provider, Reporter, SourceDigest, TaskIntent, ToolchainProber};

use crate::commands::selection::TaskSelection;
use crate::commands::support::{LiveApplyBinding, build_live_apply_host};
use crate::commands::watch::{LiveOutput, WatchRun, run_watch};
use crate::host::{Project, Report, new_run_id};
use crate::report::exit_code;

/// Whether a task run should enter watch mode, and its debounce window.
///
/// Bundled so the task-APPLY verbs thread one value instead of a bool/ms pair.
#[derive(Debug, Clone, Copy)]
pub(crate) struct WatchFlags {
    /// Whether `--watch` was requested.
    pub(crate) enabled: bool,
    /// The resolved trailing-edge debounce window, in milliseconds.
    pub(crate) debounce_ms: u64,
}

/// Resolve the effective concurrency ceiling: the `--jobs`/`-j` override wins
/// over the `[toven].max_parallel` document setting, and `None` leaves the
/// engine default (available parallelism).
pub(crate) fn resolve_max_parallel(jobs: Option<usize>, project: &Project) -> Option<usize> {
    jobs.or_else(|| project.max_parallel())
}

/// Run a task (`toven <task>` / `toven run <task>`), optionally PLAN-only.
///
/// Builds the request, emits [`Event::RunStarted`], runs the PLAN spine, and —
/// unless `plan_only` — drives APPLY to completion. Returns the process exit
/// derived from the terminal [`RunStats`].
///
/// # Errors
/// Propagates PLAN/APPLY failures and runtime construction failures. A stop
/// signal (SIGINT/SIGTERM/SIGHUP) is handled cooperatively by APPLY and
/// returned as a terminal run summary; a second signal force-exits.
#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
pub(crate) fn execute(
    providers: &[&dyn Provider],
    project: &Project,
    report: Report,
    intent: TaskIntent,
    passthrough: Vec<String>,
    fail_fast: bool,
    no_cache: bool,
    refresh: bool,
    unit_timeout: Option<std::time::Duration>,
    plan_only: bool,
    watch: WatchFlags,
    view: Option<ViewMode>,
    jobs: Option<usize>,
    selection: &TaskSelection,
) -> AppResult<ExitCode> {
    let run_id = new_run_id()?;
    let intent_name = intent.name().to_string();
    let effective_view = view.unwrap_or(project.document.toven.view);
    // Per-run scratch for the tmux pane launcher. A randomized, 0700, owned
    // `TempDir` (rather than a predictable `toven-panes-{run_id}-{pid}` path in
    // the world-writable system temp dir) closes the pre-create/read/interfere
    // window a local attacker could otherwise exploit against a guessable path.
    // The guard owns the directory for the whole run and removes it on drop,
    // however the run exits, so no manual reclaim is needed.
    let pane_scratch = rskit_fs::TempDir::new()?;
    let pane_dir = pane_scratch.path().to_path_buf();
    let mut request = PlanRequest::new(
        run_id.clone(),
        project.document.project.name.clone(),
        intent,
        project.project_root.clone(),
    )
    .with_passthrough(passthrough)
    .with_selection(selection.resolve(project.document.project.base_ref.as_deref())?);
    // `--no-cache` bypasses the cache entirely (no read, no write); `--refresh`
    // forces a re-run but still writes the fresh result. They are mutually
    // exclusive (rejected upstream), so at most one arm applies.
    if no_cache {
        request = request.with_cache_mode(CacheMode::Disabled);
    } else if refresh {
        request = request.with_cache_mode(CacheMode::Force);
    }

    let opened = project.open_member_vcs(providers, &selection.baseline)?;
    let readers = opened.readers();
    let digest = FsSourceDigest::new(&project.project_root);
    let cache = FsContentCache::new(project.cache_root()?);

    // Composition root: one shared process supervisor drives every child spawned
    // across PLAN and APPLY. Injecting it into the prober's tool runner means a
    // stop signal during a toolchain probe reaps the probe child through the same
    // backstop the APPLY runner uses, rather than orphaning it under the OS
    // default action.
    let supervisor = Arc::new(ProcessSupervisor::new(LifecyclePolicy::default()));
    let prober = ProcessToolchainProber::new(Arc::new(
        ProcessToolRunner::new().with_supervisor(Arc::clone(&supervisor)),
    ));

    // A multi-threaded runtime so the spawned signal watcher and the supervisor's
    // shutdown subscription run on worker threads: they keep observing a stop
    // signal (and reap) even while PLAN's synchronous, blocking toolchain probe
    // holds the block-on thread. Installing shutdown before PLAN then closes the
    // gap where a probe could otherwise be orphaned under the OS default action.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(AppError::internal)?;

    let mut reporter = report.reporter();
    let sink: &mut dyn Reporter = reporter.as_mut();

    let outcome = runtime.block_on(run_supervised(
        TaskRun {
            providers,
            project,
            report: &report,
            request: &request,
            readers: &readers,
            digest: &digest,
            prober: &prober,
            cache: &cache,
            supervisor: &supervisor,
            effective_view,
            pane_dir,
            run_id,
            intent_name,
            fail_fast,
            unit_timeout,
            jobs,
            plan_only,
            watch,
        },
        sink,
    ));
    // `pane_scratch` (the owned `TempDir`) removes the pane scratch dir on drop,
    // however the run exited — no manual reclaim needed.
    drop(pane_scratch);
    outcome
}

/// The borrowed inputs one supervised task run drives PLAN→APPLY with.
///
/// Bundled as one value (rather than a long positional argument list) so the
/// composition root threads the plan ports, the shared supervised lifecycle
/// (`supervisor`), and the run options through a single request into
/// [`run_supervised`].
struct TaskRun<'a> {
    /// The ecosystem adapters compiled into this binary.
    providers: &'a [&'a dyn Provider],
    /// The resolved project (document + roots).
    project: &'a Project,
    /// The reporting context, for the resolved stream/palette live-output inputs.
    report: &'a Report,
    /// The typed PLAN request.
    request: &'a PlanRequest,
    /// Per-member git seams for changed-path detection.
    readers: &'a MemberVcsReaders<'a>,
    /// Content digest for module/source cache identities.
    digest: &'a dyn SourceDigest,
    /// Toolchain version prober for active workspaces.
    prober: &'a dyn ToolchainProber,
    /// Cache store + writer for per-unit verdicts.
    cache: &'a FsContentCache,
    /// The shared process supervisor the tool and command runners register with.
    supervisor: &'a Arc<ProcessSupervisor>,
    /// The resolved view preference (`--view` over `[toven].view`).
    effective_view: ViewMode,
    /// The per-run tmux pane scratch directory (owned by the caller's `TempDir`).
    pane_dir: std::path::PathBuf,
    /// The run identity emitted on `RunStarted`.
    run_id: String,
    /// The intent name emitted on `RunStarted`.
    intent_name: String,
    /// Whether the run stops the wave on the first failure.
    fail_fast: bool,
    /// The optional per-unit wall-clock bound (`--timeout`).
    unit_timeout: Option<std::time::Duration>,
    /// The `--jobs`/`-j` concurrency override.
    jobs: Option<usize>,
    /// Whether the run stops at PLAN (`--dry-run`/`--explain`).
    plan_only: bool,
    /// Whether the run enters watch mode, and its debounce window.
    watch: WatchFlags,
}

/// Install graceful shutdown at the composition root, then drive PLAN→APPLY (or
/// the watch loop) under the caller's runtime.
///
/// Shutdown is installed and the shared supervisor subscribed *before* PLAN, so
/// a stop signal during a toolchain probe cancels and reaps rather than falling
/// through to the OS default action. The multi-threaded runtime keeps the
/// spawned signal watcher and supervisor subscription running on worker threads
/// while PLAN's synchronous, blocking probe holds the block-on thread.
///
/// # Errors
/// Propagates PLAN/APPLY failures and shutdown-installation failures.
#[allow(clippy::future_not_send)]
async fn run_supervised(run: TaskRun<'_>, sink: &mut dyn Reporter) -> AppResult<ExitCode> {
    // Install caller-declared graceful shutdown at the composition root, before
    // PLAN. The default policy captures every stop signal — SIGINT (Ctrl+C),
    // SIGTERM (`kill`/IDE-stop), and SIGHUP (terminal-close/SSH-drop) — and cancels
    // a shared token; a second signal force-exits with code 130. The token is
    // threaded into PLAN's prober and APPLY (not raced against them): on the first
    // signal the engine SIGTERMs every in-flight worker, tears down held processes,
    // and returns a normal `RunStats`. Subscribing the shared supervisor to the
    // same token is the backstop — a process-level signal reaps the whole
    // `cargo`/`nextest`/`rustc` group (and any in-flight probe) even if no
    // individual future observes it, so nothing is left holding Cargo's lock.
    let shutdown = ShutdownController::install(ShutdownPolicy::default())?;
    let cancel = shutdown.token();
    let _shutdown_backstop = run.supervisor.subscribe_shutdown(cancel.clone())?;

    if run.watch.enabled {
        return run_watch(
            WatchRun {
                providers: run.providers,
                project: run.project,
                request: run.request,
                readers: run.readers,
                digest: run.digest,
                prober: run.prober,
                cache: run.cache,
                supervisor: run.supervisor,
                fail_fast: run.fail_fast,
                unit_timeout: run.unit_timeout,
                jobs: run.jobs,
                debounce_ms: run.watch.debounce_ms,
                live: &LiveOutput {
                    view: run.effective_view,
                    force_stream: run.report.forces_stream_output(),
                    palette: run.report.stderr_palette(),
                    pane_dir: run.pane_dir,
                },
                cancel,
            },
            sink,
        )
        .await;
    }

    // Defer the run header (and the PLAN-phase events) into a buffer until PLAN
    // commits: an unresolvable task fails during scheduling, and emitting the
    // `run <task> on <repo>` header first would leave it above the error for a run
    // that never started. On success the buffer replays in emission order, so a
    // healthy run reads exactly as before.
    let mut buffered = PlanReporter::new(sink);
    let host = PlanHost::new(run.readers, run.digest, run.prober, run.cache);
    let plan = match plan(run.request, &run.project.document, run.providers, host, &mut buffered) {
        Ok(plan) => plan,
        Err(error) => {
            buffered.abort()?;
            return Err(error);
        }
    };
    buffered.commit(&Event::RunStarted {
        run_id: run.run_id,
        intent: run.intent_name,
        project: run.project.document.project.name.clone(),
    })?;

    if run.plan_only {
        let summary = plan_summary(&plan);
        sink.emit(&Event::RunFinished { summary })?;
        return Ok(exit_code(&summary));
    }

    // Resolve the effective concurrency ceiling before binding the live view:
    // `auto` streams inline for a serial (`--jobs 1`) or single-unit run, so the
    // renderer must know the ceiling the units will actually run under.
    let mut options = ApplyOptions {
        fail_fast: run.fail_fast,
        unit_timeout: run.unit_timeout,
        ..ApplyOptions::default()
    };
    if let Some(max_parallel) = resolve_max_parallel(run.jobs, run.project) {
        options.max_parallel = max_parallel.max(1);
    }

    // Bind the resolved live view (tiles/panes/stream) to a raw-output sink and the
    // PTY sizing live units run under, binding the runner to the shared supervisor.
    // The machine JSON projection, a non-terminal stderr, and `--view stream` all
    // pin the byte-stable stream shape; tiles/panes require a Unix PTY (a no-op
    // elsewhere).
    let host = build_live_apply_host(
        run.project,
        run.supervisor,
        &LiveApplyBinding {
            view: run.effective_view,
            force_stream: run.report.forces_stream_output(),
            palette: run.report.stderr_palette(),
            unit_count: plan.units.len(),
            max_parallel: options.max_parallel,
            pane_dir: &run.pane_dir,
        },
    )?;
    let runner = host.runner;
    let mut output = host.output;
    let summary = apply(&plan, runner, run.cache, sink, &mut output, options, cancel).await?;
    Ok(exit_code(&summary))
}

/// Synthesize the terminal [`RunStats`] from a PLAN-only [`Plan`].
///
/// PLAN never executes, so the summary is purely the planned unit count plus
/// the per-unit cache verdicts the planner already decided. The CLI emits this
/// as the [`Event::RunFinished`] for dry-run/explain so the one exit-mapping
/// path applies uniformly.
#[must_use]
pub(crate) fn plan_summary(plan: &Plan) -> RunStats {
    let mut summary = RunStats::new(plan.units.len());
    // A PLAN-only cut executes nothing; mark it so the summary reads as a dry run
    // rather than a real run in which every unit happened to be a cache hit.
    summary.dry_run = true;
    for unit in &plan.units {
        match unit.cache {
            CacheVerdict::Hit => {
                summary.cache_hits += 1;
                summary.cached_units += 1;
            }
            CacheVerdict::Miss => summary.cache_misses += 1,
            CacheVerdict::Disabled => summary.cache_disabled += 1,
            CacheVerdict::Forced => summary.cache_forced += 1,
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use toven_model::{
        CacheVerdict, EcosystemId, ExecutionReadiness, ExecutionUnit, ModuleKey, ModuleRef, Plan,
    };

    use super::plan_summary;

    fn unit(id: &str, cache: CacheVerdict) -> ExecutionUnit {
        ExecutionUnit {
            id: id.to_string(),
            module: ModuleKey::bare(
                ModuleRef::new(EcosystemId::new("rust").unwrap(), "core").unwrap(),
            ),
            members: vec![ModuleKey::bare(
                ModuleRef::new(EcosystemId::new("rust").unwrap(), "core").unwrap(),
            )],
            task: "build".to_string(),
            origin: toven_model::TaskOrigin::AdapterDefault,
            workspace: None,
            argv: vec!["cargo".to_string(), "build".to_string()],
            persistent: false,
            readiness: ExecutionReadiness::Started,
            readiness_timeout: Duration::from_secs(30),
            fail_if_output: false,
            cache,
            cache_key: None,
            depends_on: Vec::new(),
            resource_group: None,
        }
    }

    #[test]
    fn plan_summary_counts_each_cache_verdict() {
        let plan = Plan::new(
            vec![
                unit("a", CacheVerdict::Hit),
                unit("b", CacheVerdict::Miss),
                unit("c", CacheVerdict::Disabled),
                unit("d", CacheVerdict::Forced),
            ],
            Vec::new(),
        );

        let summary = plan_summary(&plan);

        assert_eq!(summary.planned_units, 4);
        assert_eq!(summary.cache_hits, 1);
        assert_eq!(summary.cached_units, 1);
        assert_eq!(summary.cache_misses, 1);
        assert_eq!(summary.cache_disabled, 1);
        assert_eq!(summary.cache_forced, 1);
    }

    #[test]
    fn plan_summary_of_empty_plan_is_zeroed() {
        let summary = plan_summary(&Plan::new(Vec::new(), Vec::new()));
        assert_eq!(summary.planned_units, 0);
        assert_eq!(summary.cache_hits, 0);
    }
}
