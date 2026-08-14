//! Watch mode: drive the engine's [`WatchSession`] PLAN→APPLY rerun loop.
//!
//! `toven <task> --watch` runs the task once, then reruns the affected subgraph
//! each time a source file changes. The composition root ([`run::execute`])
//! owns the Tokio runtime, the shared process supervisor, and the installed
//! graceful shutdown (SIGINT/SIGTERM/SIGHUP → cooperative teardown, backed by
//! the supervisor); this module builds the same rskit-backed APPLY host — a
//! process runner bound to that supervisor plus a per-unit output channel — and
//! the concrete [`RskitFsWatch`](toven_engine::watch::RskitFsWatch) adapter,
//! then drives the engine loop under the caller's runtime and cancellation
//! token.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rskit_cli::{CancellationToken, ExitCode, Palette};
use rskit_errors::AppResult;
use rskit_process::ProcessSupervisor;
use toven_core::config::ViewMode;
use toven_core::federation::MemberVcsReaders;
use toven_core::plan::PlanRequest;
use toven_engine::apply::ApplyOptions;
use toven_engine::cache::FsContentCache;
use toven_engine::watch::{RskitFsWatch, WatchSession};
use toven_ports::{Provider, Reporter, SourceDigest, ToolchainProber};

use crate::commands::support::{LiveApplyBinding, build_live_apply_host};
use crate::host::Project;
use crate::report::exit_code;

/// The resolved live-output binding for a watched run: which view to render and
/// the color/PTY inputs the sink needs, carried as one value so the watch host
/// mirrors [`run`](super::run)'s sink selection.
pub(crate) struct LiveOutput {
    /// The resolved view preference (`--view` over `[toven].view`).
    pub(crate) view: ViewMode,
    /// Pin the byte-stable stream shape (set for the JSON projection).
    pub(crate) force_stream: bool,
    /// The stderr palette for verdict coloring.
    pub(crate) palette: Palette,
    /// Where the tmux pane launcher keeps per-unit temp files.
    pub(crate) pane_dir: PathBuf,
}

/// The injected inputs a watched run drives its PLAN→APPLY loop with.
///
/// Bundled as one value (rather than a long positional argument list) so the
/// composition root threads the plan ports, the shared supervised lifecycle
/// (`supervisor` + `cancel`), and the run options through a single request.
pub(crate) struct WatchRun<'a> {
    /// The ecosystem adapters compiled into this binary.
    pub(crate) providers: &'a [&'a dyn Provider],
    /// The resolved project (document + roots).
    pub(crate) project: &'a Project,
    /// The template PLAN request each iteration clones.
    pub(crate) request: &'a PlanRequest,
    /// Per-member git seams for changed-path detection.
    pub(crate) readers: &'a MemberVcsReaders<'a>,
    /// Content digest for module/source cache identities.
    pub(crate) digest: &'a dyn SourceDigest,
    /// Toolchain version prober for active workspaces.
    pub(crate) prober: &'a dyn ToolchainProber,
    /// Cache store + writer for per-unit verdicts.
    pub(crate) cache: &'a FsContentCache,
    /// The shared process supervisor the runner registers spawned children with.
    pub(crate) supervisor: &'a Arc<ProcessSupervisor>,
    /// Whether the run stops the wave on the first failure.
    pub(crate) fail_fast: bool,
    /// The optional per-unit wall-clock bound (`--timeout`).
    pub(crate) unit_timeout: Option<Duration>,
    /// The `--jobs`/`-j` concurrency override.
    pub(crate) jobs: Option<usize>,
    /// The resolved trailing-edge debounce window, in milliseconds.
    pub(crate) debounce_ms: u64,
    /// The resolved live-output binding for the session.
    pub(crate) live: &'a LiveOutput,
    /// The composition root's shared cancellation token: a stop signal cancels
    /// the in-flight run and breaks the watch loop.
    pub(crate) cancel: CancellationToken,
}

/// Run a task under watch mode until a stop signal or the watcher stops.
///
/// Builds the APPLY host (bound to the caller's shared supervisor) and the
/// rskit-fs watch adapter, then drives [`WatchSession`] under the composition
/// root's runtime and cancellation token. Returns the process exit derived from
/// the last iteration's summary.
///
/// # Errors
/// Propagates PLAN/APPLY failures and watch-source initialization failures.
#[allow(clippy::future_not_send)]
pub(crate) async fn run_watch(run: WatchRun<'_>, sink: &mut dyn Reporter) -> AppResult<ExitCode> {
    // Resolve the concurrency ceiling first: `auto` streams inline for a serial
    // (`--jobs 1`) watch session, so the renderer must know the ceiling.
    let mut apply_options = ApplyOptions {
        fail_fast: run.fail_fast,
        unit_timeout: run.unit_timeout,
        ..ApplyOptions::default()
    };
    if let Some(max_parallel) = super::run::resolve_max_parallel(run.jobs, run.project) {
        apply_options.max_parallel = max_parallel.max(1);
    }
    // Bind the resolved live view for the whole watch session. The affected-set
    // size varies per rerun, so the unit count is unknown here (passed as `0`):
    // `auto` therefore resolves to tiles rather than panes, while an explicit
    // `--view panes` still self-caps, keeping a large rerun bounded.
    let host = build_live_apply_host(
        run.project,
        run.supervisor,
        &LiveApplyBinding {
            view: run.live.view,
            force_stream: run.live.force_stream,
            palette: run.live.palette,
            unit_count: 0,
            max_parallel: apply_options.max_parallel,
            pane_dir: &run.live.pane_dir,
        },
    )?;
    let runner = host.runner;
    let mut output = host.output;
    let watch = RskitFsWatch::new();

    // Graceful shutdown is installed once at the composition root, before PLAN;
    // the shared `cancel` cancels the in-flight run and breaks the watch loop, so
    // a single interrupt exits cleanly with the last summary (a second signal
    // force-exits with code 130), and the supervisor the runner is bound to reaps
    // the whole `cargo`/`nextest`/`rustc` group as the backstop.
    let summary = WatchSession {
        request: run.request.clone(),
        document: &run.project.document,
        providers: run.providers,
        readers: run.readers,
        digest: run.digest,
        prober: run.prober,
        cache_store: run.cache,
        cache_writer: run.cache,
        runner,
        apply_options,
        watch: &watch,
        debounce: Duration::from_millis(run.debounce_ms),
        reporter: sink,
        output: &mut output,
        cancel: run.cancel,
    }
    .run()
    .await;
    // The pane scratch dir is an owned `TempDir` held by the calling `run::execute`
    // for the whole session; it is reclaimed on that guard's drop, so the watch
    // host does not remove it here.
    Ok(exit_code(&summary?))
}
