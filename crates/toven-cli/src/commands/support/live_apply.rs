//! The shared live-output APPLY host the task-execution verbs build.
//!
//! [`build_live_apply_host`] assembles the one bundle `run` and `watch` both
//! need to drive the engine APPLY loop: a [`ProcessCommandRunner`] bound to the
//! resolved live view (tiles/panes/stream) and to the caller-owned process
//! [`ProcessSupervisor`], plus a per-unit [`UnitOutputChannel`] over the
//! selected raw-output sink. The Tokio runtime and the supervisor are owned by
//! the composition root (the execution verb) and injected, so `run` and `watch`
//! drive PLAN and APPLY under one supervised lifecycle rather than each host
//! standing up its own runtime and supervisor.

use std::path::Path;
use std::sync::Arc;

use rskit_cli::Palette;
use rskit_errors::AppResult;
use rskit_process::ProcessSupervisor;
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

/// The assembled live-output APPLY host: the injected command runner and the
/// per-unit output channel the verb drives its APPLY loop with.
///
/// The runtime and the shared [`ProcessSupervisor`] are owned by the
/// composition root (the execution verb) and injected, so the runner reported
/// here is already bound to the supervisor the root subscribes to the installed
/// shutdown handle.
pub(crate) struct LiveApplyHost {
    /// The live-view-bound process runner, ready to hand to the engine.
    pub(crate) runner: Arc<dyn CommandRunner>,
    /// The per-unit output channel the engine emits raw bytes through.
    pub(crate) output: UnitOutputChannel<Box<dyn RawOutputSink>>,
}

/// Build the shared live-output APPLY host for `project` under `binding`,
/// binding its process runner to the caller-owned `supervisor`.
///
/// The injected `supervisor` is the one the composition root subscribes to the
/// installed shutdown handle, so a process-level stop reaps every child this
/// runner spawns as the backstop behind cooperative cancellation.
///
/// # Errors
/// Propagates a live-view binding failure (PTY/pane setup).
pub(crate) fn build_live_apply_host(
    project: &Project,
    supervisor: &Arc<ProcessSupervisor>,
    binding: &LiveApplyBinding<'_>,
) -> AppResult<LiveApplyHost> {
    let (configured_runner, raw_sink) = configure_live_output(
        ProcessCommandRunner::new(project.project_root.as_path())
            .with_supervisor(Arc::clone(supervisor)),
        binding.view,
        binding.force_stream,
        binding.palette,
        binding.unit_count,
        binding.max_parallel,
        binding.pane_dir,
    )?;
    let runner: Arc<dyn CommandRunner> = Arc::new(configured_runner);
    let output = UnitOutputChannel::new(raw_sink);
    Ok(LiveApplyHost { runner, output })
}
