//! [`WatchSession`]: the watch-mode PLAN→APPLY loop.
//!
//! Watch mode reruns the affected subgraph each time the workspace tree
//! changes. The session runs one baseline iteration, then drives the injected
//! [`WatchSource`] stream: every debounced batch is relativized against the
//! workspace root, dropping paths inside `.git` and paths the root repo
//! ignores, and — when anything remains — mapped to a
//! [`Selection::ChangedPaths`] PLAN request that plans and applies exactly the
//! affected units. When a batch reports a rescan (the watcher dropped events),
//! the incomplete path list is discarded and the whole watched scope (the
//! caller's baseline selection) is re-evaluated instead. The shared
//! [`CancellationToken`] both cancels an in-flight run and breaks the loop, so
//! a single Ctrl+C exits cleanly with the last iteration's summary.

use std::ffi::OsStr;
use std::path::{Component, Path};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt as _;
use rskit_errors::AppResult;
use tokio_util::sync::CancellationToken;
use toven_model::{AbsPath, Event, RunStats};
use toven_ports::{
    ChangeBatch, CommandRunner, PlanReporter, Provider, Reporter, SourceDigest, ToolchainProber,
    WatchSource,
};

use crate::apply::{ApplyOptions, apply};
use crate::output::UnitOutputChannel;
use toven_engine_core::config::Document;
use toven_engine_core::federation::baseline::MemberVcsReaders;
use toven_engine_core::plan::{PlanHost, PlanRequest, Selection, plan};

use toven_ports::{CacheStore, CacheWriter, RawOutputSink};

/// One watch-mode run: the injected ports plus the mutable event/output sinks,
/// driven by [`WatchSession::run`].
///
/// The `request` field is the per-iteration template — every iteration clones
/// it with a fresh run id and its own [`Selection`], so the baseline run keeps
/// the caller's selection while change-driven runs narrow to the affected
/// paths.
pub struct WatchSession<'a, S: RawOutputSink> {
    /// PLAN request template (project root, intent, passthrough, cache mode).
    pub request: PlanRequest,
    /// The strict configuration document.
    pub document: &'a Document,
    /// The compiled-in ecosystem providers.
    pub providers: &'a [&'a dyn Provider],
    /// Per-member VCS readers (ignore filtering + PLAN change selection).
    pub readers: &'a MemberVcsReaders<'a>,
    /// The injected content-digest port.
    pub digest: &'a dyn SourceDigest,
    /// The injected toolchain-probe port.
    pub prober: &'a dyn ToolchainProber,
    /// The cache read port consulted during PLAN.
    pub cache_store: &'a dyn CacheStore,
    /// The cache write port recorded during APPLY.
    pub cache_writer: &'a dyn CacheWriter,
    /// The process-execution port shared across iterations.
    pub runner: Arc<dyn CommandRunner>,
    /// APPLY runtime knobs, cloned per iteration.
    pub apply_options: ApplyOptions,
    /// The injected filesystem-watch port.
    pub watch: &'a dyn WatchSource,
    /// Trailing-edge debounce window for coalescing filesystem events.
    pub debounce: Duration,
    /// The event sink both PLAN and APPLY emit through.
    pub reporter: &'a mut dyn Reporter,
    /// The per-unit raw child-output channel.
    pub output: &'a mut UnitOutputChannel<S>,
    /// Shared cooperative cancellation (Ctrl+C): cancels the run and exits.
    pub cancel: CancellationToken,
}

impl<S: RawOutputSink> WatchSession<'_, S> {
    /// Drive the watch loop until cancelled or the watcher stops.
    ///
    /// Emits [`Event::WatchStarted`] once, runs a baseline iteration, then
    /// reruns the affected subgraph per debounced change batch, and emits
    /// [`Event::WatchStopped`] on exit. A path-driven batch emits
    /// [`Event::WatchTriggered`]; a rescan batch (dropped events) emits
    /// [`Event::WatchRescan`] and re-evaluates the baseline scope. Returns the
    /// last iteration's summary (the baseline's when no change ever triggered a
    /// rerun).
    ///
    /// # Errors
    /// Propagates PLAN/APPLY failures and watch-source initialization failures.
    /// Non-zero child exits are represented in the returned [`RunStats`].
    ///
    /// Not `Send`: the injected reporter and ports are single-threaded and the
    /// CLI drives this on a current-thread runtime, so the future never crosses
    /// a thread boundary.
    #[allow(clippy::future_not_send)]
    pub async fn run(mut self) -> AppResult<RunStats> {
        let debounce_ms = u64::try_from(self.debounce.as_millis()).unwrap_or(u64::MAX);

        let roots = watch_roots(&self.request.project_root);
        let mut stream = self
            .watch
            .changes(&roots, self.debounce, self.cancel.clone())?;
        self.reporter.emit(&Event::WatchStarted { debounce_ms })?;

        // Baseline iteration: the caller's selection (a full run by default).
        let baseline = self.iteration_request(0, self.request.selection.clone());
        let mut summary = self.iterate(&baseline).await?;

        let mut iteration: u64 = 1;
        loop {
            let batch = tokio::select! {
                () = self.cancel.cancelled() => break,
                next = stream.next() => match next {
                    Some(batch) => batch,
                    // The watcher stopped (torn down or errored): end the loop.
                    None => break,
                },
            };

            // Overflow: the change list is incomplete, so re-evaluate the whole watched
            // scope (the caller's baseline selection) rather than trust the partial paths.
            let selection = if batch.rescan_requested() {
                self.reporter.emit(&Event::WatchRescan)?;
                self.request.selection.clone()
            } else {
                let paths = self.changed_paths(&batch)?;
                if paths.is_empty() {
                    continue;
                }
                self.reporter.emit(&Event::WatchTriggered {
                    paths: paths.clone(),
                })?;
                Selection::ChangedPaths(paths)
            };

            let request = self.iteration_request(iteration, selection);
            summary = self.iterate(&request).await?;
            iteration += 1;
        }

        self.reporter.emit(&Event::WatchStopped)?;
        Ok(summary)
    }

    /// Clone the template into a per-iteration request with a fresh run id and
    /// the iteration's selection.
    fn iteration_request(&self, iteration: u64, selection: Selection) -> PlanRequest {
        let mut request = self.request.clone();
        request.run_id = format!("{}-{iteration}", self.request.run_id);
        request.selection = selection;
        request
    }

    /// Run one PLAN→APPLY iteration, emitting the run's lifecycle events.
    #[allow(clippy::future_not_send)]
    async fn iterate(&mut self, request: &PlanRequest) -> AppResult<RunStats> {
        let host = PlanHost::new(self.readers, self.digest, self.prober, self.cache_store);
        let mut buffered = PlanReporter::new(self.reporter);
        let plan = match plan(request, self.document, self.providers, host, &mut buffered) {
            Ok(plan) => plan,
            Err(error) => {
                buffered.abort()?;
                return Err(error);
            }
        };
        buffered.commit(&Event::RunStarted {
            run_id: request.run_id.clone(),
            intent: request.intent.name().to_string(),
            project: request.project.clone(),
        })?;
        apply(
            &plan,
            Arc::clone(&self.runner),
            self.cache_writer,
            self.reporter,
            self.output,
            self.apply_options.clone(),
            self.cancel.clone(),
        )
        .await
    }

    /// Relativize a batch against the workspace root, dropping paths outside
    /// the root, inside `.git`, or ignored by the root repo.
    ///
    /// The returned paths are workspace-root-relative, sorted, and
    /// deduplicated, ready to seed [`Selection::ChangedPaths`].
    fn changed_paths(&self, batch: &ChangeBatch) -> AppResult<Vec<String>> {
        let root = self.request.project_root.as_path();
        let mut paths = Vec::new();
        for absolute in batch.paths() {
            let Ok(relative) = absolute.strip_prefix(root) else {
                continue;
            };
            if relative.as_os_str().is_empty() || in_git_dir(relative) {
                continue;
            }
            if self.is_ignored(relative)? {
                continue;
            }
            paths.push(relative.to_string_lossy().into_owned());
        }
        paths.sort();
        paths.dedup();
        Ok(paths)
    }

    /// Whether the root repo ignores `relative` (workspace-root-relative).
    ///
    /// Only the degenerate/root member's reader (the entry with no
    /// [`member`](MemberVcsReader::member) id, sitting at the workspace root)
    /// is consulted: its ignore rules are the ones that apply to root-relative
    /// paths. In umbrella/federated setups the per-member readers are scoped to
    /// their own repo roots, so consulting an arbitrary member would risk false
    /// positives/negatives across repos. With no root reader (a non-git host,
    /// or a pure umbrella) nothing is treated as ignored, so every change still
    /// drives a rerun.
    fn is_ignored(&self, relative: &Path) -> AppResult<bool> {
        self.readers
            .entries()
            .iter()
            .find(|entry| entry.member().is_none())
            .map_or_else(|| Ok(false), |entry| entry.reader().is_ignored(relative))
    }
}

/// Resolve `roots` to the absolute workspace roots a watch observes.
///
/// Currently the single workspace root; kept as a helper so federation can grow
/// to multiple member roots without reshaping the call site.
#[must_use]
pub fn watch_roots(project_root: &AbsPath) -> Vec<AbsPath> {
    vec![project_root.clone()]
}

/// Whether a workspace-relative path lies inside a `.git` directory.
fn in_git_dir(relative: &Path) -> bool {
    relative
        .components()
        .any(|component| matches!(component, Component::Normal(name) if name == OsStr::new(".git")))
}
