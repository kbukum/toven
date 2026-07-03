//! Watch mode: drive the engine's [`WatchSession`] PLAN→APPLY rerun loop.
//!
//! `toven <task> --watch` runs the task once, then reruns the affected subgraph
//! each time a source file changes. This module builds the same rskit-backed
//! APPLY host as [`run`](super::run) — process runner, per-unit output channel,
//! and cooperative Ctrl+C cancellation — plus the concrete
//! [`RskitFsWatch`](toven_engine::watch::RskitFsWatch) adapter, then hands them to
//! the engine loop on a Tokio runtime.

use std::sync::Arc;
use std::time::Duration;

use rskit_cli::{ExitCode, on_ctrl_c};
use rskit_errors::{AppError, AppResult};
use toven_engine::apply::{ApplyOptions, ProcessCommandRunner};
use toven_engine::cache::FsContentCache;
use toven_engine::federation::MemberVcsReaders;
use toven_engine::output::UnitOutputChannel;
use toven_engine::plan::PlanRequest;
use toven_engine::watch::{RskitFsWatch, WatchSession};
use toven_ports::{CommandRunner, Provider, Reporter, SourceDigest, ToolchainProber};

use crate::host::Project;
use crate::report::{WriterRawSink, exit_code};

/// Run a task under watch mode until Ctrl+C or the watcher stops.
///
/// Builds the APPLY host and the rskit-fs watch adapter, then drives
/// [`WatchSession`] on a current-thread runtime. Returns the process exit derived
/// from the last iteration's summary.
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
    debounce_ms: u64,
    sink: &mut dyn Reporter,
) -> AppResult<ExitCode> {
    let runner: Arc<dyn CommandRunner> =
        Arc::new(ProcessCommandRunner::new(project.project_root.as_path()));
    let mut apply_options = ApplyOptions {
        fail_fast,
        ..ApplyOptions::default()
    };
    if let Some(max_parallel) = project.max_parallel() {
        apply_options.max_parallel = max_parallel.max(1);
    }
    let mut output = UnitOutputChannel::new(WriterRawSink::stderr());
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
    })?;
    Ok(exit_code(&summary))
}
