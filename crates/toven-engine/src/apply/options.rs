//! [`ApplyOptions`] — runtime knobs for the APPLY spine.

use toven_ports::InvocationEnvironment;

/// The `PATH` environment variable, forwarded into the default explicit
/// invocation environment so resolved programs remain discoverable.
pub(super) const PATH_ENV: &str = "PATH";

/// Default bound on the live raw-output bridge (see
/// [`ApplyOptions::live_output_capacity`]). Generous enough to absorb ordinary
/// bursts without backpressuring well-behaved processes, small enough to keep
/// the bridge bounded under a persistently slow consumer.
const DEFAULT_LIVE_OUTPUT_CAPACITY: usize = 1024;

/// Runtime options for APPLY.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ApplyOptions {
    /// Maximum units executing concurrently.
    pub max_parallel: usize,
    /// Cancel in-flight work and stop scheduling after the first failure.
    pub fail_fast: bool,
    /// Environment policy used for every task command.
    pub environment: InvocationEnvironment,
    /// Bound on the live raw-output bridge between persistent process reader
    /// threads and the APPLY consumer.
    ///
    /// Persistent units can emit output indefinitely; a bounded channel keeps
    /// the bridge from growing without limit when the consumer (e.g. a slow
    /// [`RawOutputSink`](toven_ports::RawOutputSink)) falls behind. When the
    /// channel is full the producing reader thread blocks, applying backpressure
    /// down to the child process's pipe rather than buffering or dropping
    /// output. The consumer drains the bridge continuously while units run and
    /// concurrently during teardown, so a full channel never deadlocks a process
    /// awaiting shutdown.
    pub live_output_capacity: usize,
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
            live_output_capacity: DEFAULT_LIVE_OUTPUT_CAPACITY,
        }
    }
}
