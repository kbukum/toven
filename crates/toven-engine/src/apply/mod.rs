//! APPLY spine: wave-driven exec, failure gating, cache recording, and teardown.

use std::sync::Arc;

use rskit_errors::AppResult;
use toven_model::{Plan, RunStats};
use toven_ports::{
    CacheWriter, CommandRunner, InvocationEnvironment, OutputObserver, RawOutputSink, Reporter,
};

use crate::output::UnitOutputChannel;

mod exec;
mod gating;
mod persistent;
mod pool;
mod record;
mod walk;

pub use exec::ProcessCommandRunner;

/// The `PATH` environment variable, forwarded into the default explicit
/// invocation environment so resolved programs remain discoverable.
const PATH_ENV: &str = "PATH";

/// Runtime options for APPLY.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ApplyOptions {
    /// Maximum units executing concurrently.
    pub max_parallel: usize,
    /// Cancel in-flight work and stop scheduling after the first failure.
    pub fail_fast: bool,
    /// Environment policy used for every task command.
    pub environment: InvocationEnvironment,
}

impl Default for ApplyOptions {
    fn default() -> Self {
        let mut vars = std::collections::BTreeMap::new();
        if let Some(path) = rskit_util::env::get(PATH_ENV) {
            vars.insert(PATH_ENV.to_string(), path);
        }
        Self {
            max_parallel: std::thread::available_parallelism().map_or(1, std::num::NonZero::get),
            fail_fast: false,
            environment: InvocationEnvironment::explicit(vars),
        }
    }
}

/// Execute an immutable [`Plan`] and return aggregated run statistics.
///
/// # Errors
/// Propagates reporter, raw-output sink, command-runner, cache-write, pool, and
/// teardown failures. Non-zero child exits are represented in the returned
/// [`RunStats`], not as `Err`.
pub async fn apply<S: RawOutputSink + Send>(
    plan: &Plan,
    runner: Arc<dyn CommandRunner>,
    cache: &dyn CacheWriter,
    reporter: &mut dyn Reporter,
    output: &mut UnitOutputChannel<S>,
    options: ApplyOptions,
) -> AppResult<RunStats> {
    let (live_tx, live_rx) = tokio::sync::mpsc::unbounded_channel();
    let observer = OutputObserver::new(move |chunk| {
        let _ = live_tx.send(chunk);
    });
    let pool = pool::ApplyPool::new(
        runner,
        options.max_parallel,
        options.environment.clone(),
        observer,
    );
    walk::Walker::new(plan, cache, reporter, output, options)
        .run(pool, live_rx)
        .await
}
