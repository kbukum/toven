//! Execution verbs: the argv-first task and the `run <task>` escape hatch.
//!
//! These are the verbs with an APPLY half. Each builds the typed
//! [`PlanRequest`], binds the rskit-backed engine ports, emits the CLI-owned
//! [`Event::RunStarted`], and calls the engine PLAN spine. A `--dry-run` /
//! `--explain` cut stops at PLAN and synthesizes the terminal summary from the
//! immutable [`Plan`]; a full run drives APPLY on a Tokio runtime with
//! cooperative Ctrl-C cancellation. The `release` lifecycle lives in its own
//! [`commands::release`](crate::commands::release) module.

use std::sync::Arc;

use rskit_cli::{ExitCode, on_ctrl_c};
use rskit_errors::{AppError, AppResult};
use toven_engine::apply::{ApplyOptions, ProcessCommandRunner, apply};
use toven_engine::cache::FsContentCache;
use toven_engine_core::config::ViewMode;
use toven_engine::output::UnitOutputChannel;
use toven_engine_core::plan::{
    CacheMode, FsSourceDigest, PlanHost, PlanRequest, ProcessToolchainProber, plan,
};
use toven_model::{CacheVerdict, Event, Plan, RunStats};
use toven_ports::{CommandRunner, PlanReporter, Provider, Reporter, TaskIntent};

use crate::commands::selection::TaskSelection;
use crate::host::{Project, Report, new_run_id};
use crate::report::{configure_live_output, exit_code};

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
/// Propagates PLAN/APPLY failures and runtime construction failures. Ctrl+C is
/// handled cooperatively by APPLY and returned as a terminal run summary.
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
    let prober = ProcessToolchainProber::new();
    let cache = FsContentCache::new(project.cache_root()?);

    let mut reporter = report.reporter();
    let sink: &mut dyn Reporter = reporter.as_mut();

    if watch.enabled {
        return crate::commands::watch::run_watch(
            providers,
            project,
            &request,
            &readers,
            &digest,
            &prober,
            &cache,
            fail_fast,
            unit_timeout,
            jobs,
            watch.debounce_ms,
            &crate::commands::watch::LiveOutput {
                view: effective_view,
                force_stream: report.forces_stream_output(),
                palette: report.stderr_palette(),
                pane_dir,
            },
            sink,
        );
    }

    // Defer the run header (and the PLAN-phase events) into a buffer until PLAN
    // commits: an unresolvable task fails during scheduling, and emitting the
    // `run <task> on <repo>` header first would leave it above the error for a
    // run that never started. On success the buffer replays in emission order,
    // so a healthy run reads exactly as before.
    let mut buffered = PlanReporter::new(sink);
    let host = PlanHost::new(&readers, &digest, &prober, &cache);
    let plan = match plan(&request, &project.document, providers, host, &mut buffered) {
        Ok(plan) => plan,
        Err(error) => {
            buffered.abort()?;
            return Err(error);
        }
    };
    buffered.commit(&Event::RunStarted {
        run_id,
        intent: intent_name,
        project: project.document.project.name.clone(),
    })?;

    if plan_only {
        let summary = plan_summary(&plan);
        sink.emit(&Event::RunFinished { summary })?;
        return Ok(exit_code(&summary));
    }

    // Resolve the effective concurrency ceiling before binding the live view:
    // `auto` streams inline for a serial (`--jobs 1`) or single-unit run, so the
    // renderer must know the ceiling the units will actually run under.
    let mut options = ApplyOptions {
        fail_fast,
        unit_timeout,
        ..ApplyOptions::default()
    };
    if let Some(max_parallel) = resolve_max_parallel(jobs, project) {
        options.max_parallel = max_parallel.max(1);
    }

    // Bind the resolved live view (tiles/panes/stream) to a raw-output sink and the
    // PTY sizing live units run under. The machine JSON projection, a non-terminal
    // stderr, and `--view stream` all pin the byte-stable stream shape; tiles/panes
    // require a Unix PTY (a no-op elsewhere).
    let (configured_runner, raw_sink) = configure_live_output(
        ProcessCommandRunner::new(project.project_root.as_path()),
        effective_view,
        report.forces_stream_output(),
        report.stderr_palette(),
        plan.units.len(),
        options.max_parallel,
        &pane_dir,
    )?;
    let runner: Arc<dyn CommandRunner> = Arc::new(configured_runner);
    let mut output = UnitOutputChannel::new(raw_sink);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(AppError::internal)?;
    let summary = runtime.block_on(async {
        // Install Ctrl+C → cooperative cancellation. The token is threaded into APPLY
        // (not raced against it): on interrupt the engine SIGTERMs every in-flight
        // worker, tears down held processes, and returns a normal `RunStats` instead of
        // leaking child processes by dropping the future.
        let cancel = on_ctrl_c();
        apply(&plan, runner, &cache, sink, &mut output, options, cancel).await
    });
    // `pane_scratch` (the owned `TempDir`) removes the pane scratch dir on drop,
    // however the run exited — no manual reclaim needed.
    drop(pane_scratch);
    Ok(exit_code(&summary?))
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
