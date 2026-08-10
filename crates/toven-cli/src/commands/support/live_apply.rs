//! The shared live-output APPLY host the task-execution verbs build.
//!
//! [`build_live_apply_host`] assembles the one bundle `run` and `watch` both
//! need to drive the engine APPLY loop: a [`ProcessCommandRunner`] bound to the
//! resolved live view (tiles/panes/stream), a per-unit [`UnitOutputChannel`]
//! over the selected raw-output sink, and a current-thread Tokio runtime. Each
//! verb keeps its own `runtime.block_on(...)` body (task apply vs. watch loop);
//! only the wiring is shared, so there is a single place the runner, output
//! channel, and runtime are constructed.

use std::path::Path;
use std::sync::Arc;

use rskit_cli::Palette;
use rskit_errors::{AppError, AppResult};
use toven_core::config::ViewMode;
use toven_engine::apply::ProcessCommandRunner;
use toven_engine::output::UnitOutputChannel;
use toven_ports::{CommandRunner, RawOutputSink};

use crate::host::Project;
use crate::report::configure_live_output;

/// The resolved inputs the live-output APPLY host binds to.
///
/// One value carrying the view preference plus the color/PTY/scale inputs
/// [`configure_live_output`] needs, so the two execution verbs pass an
/// identically-shaped request instead of a long positional argument list.
pub(crate) struct LiveApplyBinding<'a> {
    /// The resolved view preference (`--view` over `[toven].view`).
    pub(crate) view: ViewMode,
    /// Pin the byte-stable stream shape (set for the JSON projection).
    pub(crate) force_stream: bool,
    /// The stderr palette for verdict coloring.
    pub(crate) palette: Palette,
    /// The number of units this run will drive; `0` when unknown (watch reruns
    /// vary), which resolves `auto` to tiles rather than panes.
    pub(crate) unit_count: usize,
    /// The resolved concurrency ceiling the units run under.
    pub(crate) max_parallel: usize,
    /// Where the tmux pane launcher keeps its per-unit temp files.
    pub(crate) pane_dir: &'a Path,
}

/// The assembled live-output APPLY host: the injected command runner, the
/// per-unit output channel, and the runtime the verb drives its loop on.
pub(crate) struct LiveApplyHost {
    /// The live-view-bound process runner, ready to hand to the engine.
    pub(crate) runner: Arc<dyn CommandRunner>,
    /// The per-unit output channel the engine emits raw bytes through.
    pub(crate) output: UnitOutputChannel<Box<dyn RawOutputSink>>,
    /// The current-thread runtime the verb blocks on.
    pub(crate) runtime: tokio::runtime::Runtime,
}

/// Build the shared live-output APPLY host for `project` under `binding`.
///
/// # Errors
/// Propagates a live-view binding failure (PTY/pane setup) and a runtime
/// construction failure.
pub(crate) fn build_live_apply_host(
    project: &Project,
    binding: &LiveApplyBinding<'_>,
) -> AppResult<LiveApplyHost> {
    let (configured_runner, raw_sink) = configure_live_output(
        ProcessCommandRunner::new(project.project_root.as_path()),
        binding.view,
        binding.force_stream,
        binding.palette,
        binding.unit_count,
        binding.max_parallel,
        binding.pane_dir,
    )?;
    let runner: Arc<dyn CommandRunner> = Arc::new(configured_runner);
    let output = UnitOutputChannel::new(raw_sink);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(AppError::internal)?;
    Ok(LiveApplyHost {
        runner,
        output,
        runtime,
    })
}
