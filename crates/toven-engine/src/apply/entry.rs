//! [`apply`] — the public APPLY entry point that wires the live-output bridge,
//! worker pool, and wave [`Walker`](super::walk::Walker) together.

use std::sync::Arc;

use rskit_errors::AppResult;
use tokio_util::sync::CancellationToken;
use toven_model::{Plan, RunStats};
use toven_ports::{CacheWriter, CommandRunner, OutputObserver, RawOutputSink, Reporter};

use crate::output::UnitOutputChannel;

use super::ApplyOptions;
use super::pool::ApplyPool;
use super::walk::Walker;

/// Execute an immutable [`Plan`] and return aggregated run statistics.
///
/// `cancel` is a cooperative external cancellation signal (typically wired to
/// Ctrl+C by the CLI). When it fires, the in-flight workers are sent the same
/// SIGTERM-and-wait teardown as a `--fail-fast` failure, scheduling stops, and
/// the held-process teardown and pool shutdown still run — no child process is
/// abandoned. Passing a never-cancelled token disables external cancellation.
///
/// # Errors
/// Propagates reporter, raw-output sink, command-runner, cache-write, pool, and
/// teardown failures. Non-zero child exits are represented in the returned
/// [`RunStats`], not as `Err`.
pub async fn apply<S: RawOutputSink>(
    plan: &Plan,
    runner: Arc<dyn CommandRunner>,
    cache: &dyn CacheWriter,
    reporter: &mut dyn Reporter,
    output: &mut UnitOutputChannel<S>,
    options: ApplyOptions,
    cancel: CancellationToken,
) -> AppResult<RunStats> {
    // Bounded bridge between persistent reader threads and the APPLY consumer so
    // the queue cannot grow without limit when the consumer falls behind.
    let (live_tx, live_rx) = tokio::sync::mpsc::channel(options.live_output_capacity.max(1));
    // Chunks dropped because the bridge was full and the producer could not block;
    // surfaced via `RunStats::dropped_output_chunks` so loss is never silent.
    let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observer = {
        let dropped = Arc::clone(&dropped);
        OutputObserver::new(move |chunk| {
            // On a dedicated OS reader thread (production rskit-process path) we can block,
            // applying lossless backpressure down to the child's pipe. Inside an async
            // runtime (e.g. a test/in-process producer) blocking is forbidden, so fall back
            // to a non-blocking send and account for any drop instead of deadlocking the
            // runtime.
            if tokio::runtime::Handle::try_current().is_err() {
                // Blocking send applies lossless backpressure when the bridge is full,
                // and only errors once the receiver is gone (the APPLY consumer has
                // torn down). Such a chunk is genuinely undeliverable, so count it —
                // keeping the "loss is never silent" contract honest on this path too.
                if live_tx.blocking_send(chunk).is_err() {
                    dropped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            } else if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) =
                live_tx.try_send(chunk)
            {
                dropped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        })
    };
    // Stream normal-unit output live when the sink de-interleaves concurrent output
    // by unit (one region per unit), or when no two units can run concurrently (a
    // single-unit plan or strictly serial execution). Otherwise chunks from
    // parallel units would interleave, so they stay buffered into deterministic
    // per-unit blocks.
    let stream_normal_live =
        super::options::stream_normal_live(&options, plan, output.supports_concurrent_live());
    let pool = ApplyPool::new(
        runner,
        options.max_parallel,
        options.environment.clone(),
        observer,
        stream_normal_live,
        options.unit_timeout,
    );
    Walker::new(plan, cache, reporter, output, options, dropped, cancel)
        .run(pool, live_rx)
        .await
}
