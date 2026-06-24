//! [`apply`] — the public APPLY entry point that wires the live-output bridge,
//! worker pool, and wave [`Walker`](super::walk::Walker) together.

use std::sync::Arc;

use rskit_errors::AppResult;
use toven_model::{Plan, RunStats};
use toven_ports::{CacheWriter, CommandRunner, OutputObserver, RawOutputSink, Reporter};

use crate::output::UnitOutputChannel;

use super::ApplyOptions;
use super::pool::ApplyPool;
use super::walk::Walker;

/// Execute an immutable [`Plan`] and return aggregated run statistics.
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
) -> AppResult<RunStats> {
    // Bounded bridge between persistent reader threads and the APPLY consumer so
    // the queue cannot grow without limit when the consumer falls behind.
    let (live_tx, live_rx) = tokio::sync::mpsc::channel(options.live_output_capacity.max(1));
    // Chunks dropped because the bridge was full and the producer could not
    // block; surfaced via `RunStats::dropped_output_chunks` so loss is never
    // silent.
    let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observer = {
        let dropped = Arc::clone(&dropped);
        OutputObserver::new(move |chunk| {
            // On a dedicated OS reader thread (production rskit-process path) we
            // can block, applying lossless backpressure down to the child's
            // pipe. Inside an async runtime (e.g. a test/in-process producer)
            // blocking is forbidden, so fall back to a non-blocking send and
            // account for any drop instead of deadlocking the runtime.
            if tokio::runtime::Handle::try_current().is_err() {
                let _ = live_tx.blocking_send(chunk);
            } else if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) =
                live_tx.try_send(chunk)
            {
                dropped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        })
    };
    let pool = ApplyPool::new(
        runner,
        options.max_parallel,
        options.environment.clone(),
        observer,
    );
    Walker::new(plan, cache, reporter, output, options, dropped)
        .run(pool, live_rx)
        .await
}
