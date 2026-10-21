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
    // Bounded bridge from every output producer to the APPLY consumer. Producers
    // wait for capacity, so a slow consumer applies backpressure instead of losing
    // output, and the queue cannot grow without limit.
    let (live_tx, live_rx) = tokio::sync::mpsc::channel(options.live_output_capacity.max(1));
    let observer = OutputObserver::new(live_tx);
    let dropped = observer.dropped();
    // Every producer uses the bridge; only presentation varies between live regions, buffered blocks, and quiet tails.
    let normal = super::options::normal_output(
        &options,
        plan,
        output.supports_concurrent_live(),
        output.hides_passing(),
    );
    let pool = ApplyPool::new(
        runner,
        options.max_parallel,
        options.environment.clone(),
        observer,
        normal,
        options.unit_timeout,
    );
    Walker::new(plan, cache, reporter, output, options, dropped, cancel)
        .run(pool, live_rx)
        .await
}
