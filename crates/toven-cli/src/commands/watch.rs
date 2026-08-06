//! Watch mode: drive the engine's [`WatchSession`] PLAN→APPLY rerun loop.
//!
//! `toven <task> --watch` runs the task once, then reruns the affected subgraph
//! each time a source file changes. This module builds the same rskit-backed
//! APPLY host as [`run`](super::run) — process runner, per-unit output channel,
//! and cooperative Ctrl+C cancellation — plus the concrete
//! [`RskitFsWatch`](toven_engine::watch::RskitFsWatch) adapter, then hands them
//! to the engine loop on a Tokio runtime.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rskit_cli::{ExitCode, Palette, on_ctrl_c};
use rskit_errors::{AppError, AppResult};
use toven_engine::apply::{ApplyOptions, ProcessCommandRunner};
use toven_engine::cache::FsContentCache;
use toven_engine_core::config::ViewMode;
use toven_engine_core::federation::MemberVcsReaders;
use toven_engine::output::UnitOutputChannel;
use toven_engine_core::plan::PlanRequest;
use toven_engine::watch::{RskitFsWatch, WatchSession};
use toven_ports::{CommandRunner, Provider, Reporter, SourceDigest, ToolchainProber};

use crate::host::Project;
use crate::report::{configure_live_output, exit_code};

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

/// Run a task under watch mode until Ctrl+C or the watcher stops.
///
/// Builds the APPLY host and the rskit-fs watch adapter, then drives
/// [`WatchSession`] on a current-thread runtime. Returns the process exit
/// derived from the last iteration's summary.
///
/// # Errors
/// Propagates PLAN/APPLY failures, watch-source initialization failures, and
/// runtime construction failures.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_watch(
    providers: &[&dyn Provider],
    project: &Project,
    request: &PlanRequest,
    readers: &MemberVcsReaders<'_>,
    digest: &dyn SourceDigest,
    prober: &dyn ToolchainProber,
    cache: &FsContentCache,
    fail_fast: bool,
    unit_timeout: Option<Duration>,
    jobs: Option<usize>,
    debounce_ms: u64,
    live: &LiveOutput,
    sink: &mut dyn Reporter,
) -> AppResult<ExitCode> {
    // Resolve the concurrency ceiling first: `auto` streams inline for a serial
    // (`--jobs 1`) watch session, so the renderer must know the ceiling.
    let mut apply_options = ApplyOptions {
        fail_fast,
        unit_timeout,
        ..ApplyOptions::default()
    };
    if let Some(max_parallel) = super::run::resolve_max_parallel(jobs, project) {
        apply_options.max_parallel = max_parallel.max(1);
    }
    // Bind the resolved live view for the whole watch session. The affected-set
    // size varies per rerun, so the unit count is unknown here (passed as `0`):
    // `auto` therefore resolves to tiles rather than panes, while an explicit
    // `--view panes` still self-caps, keeping a large rerun bounded.
    let (configured_runner, raw_sink) = configure_live_output(
        ProcessCommandRunner::new(project.project_root.as_path()),
        live.view,
        live.force_stream,
        live.palette,
        0,
        apply_options.max_parallel,
        &live.pane_dir,
    )?;
    let runner: Arc<dyn CommandRunner> = Arc::new(configured_runner);
    let mut output = UnitOutputChannel::new(raw_sink);
    let watch = RskitFsWatch::new();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(AppError::internal)?;
    let summary = runtime.block_on(async {
        // Ctrl+C is shared with APPLY: it cancels the in-flight run and breaks the
        // watch loop, so a single interrupt exits cleanly with the last summary.
        let cancel = on_ctrl_c();
        WatchSession {
            request: request.clone(),
            document: &project.document,
            providers,
            readers,
            digest,
            prober,
            cache_store: cache,
            cache_writer: cache,
            runner,
            apply_options,
            watch: &watch,
            debounce: Duration::from_millis(debounce_ms),
            reporter: sink,
            output: &mut output,
            cancel,
        }
        .run()
        .await
    });
    // The pane scratch dir is an owned `TempDir` held by the calling `run::execute`
    // for the whole session; it is reclaimed on that guard's drop, so the watch
    // host does not remove it here.
    Ok(exit_code(&summary?))
}
