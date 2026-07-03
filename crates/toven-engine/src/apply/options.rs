//! [`ApplyOptions`] — runtime knobs for the APPLY spine.

use std::time::Duration;

use toven_ports::InvocationEnvironment;

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
    ///
    /// Defaults to inheriting the parent process environment so spawned
    /// toolchains (cargo, go, git, …) see the variables they rely on — `HOME`,
    /// `CARGO_HOME`, `RUSTUP_HOME`, `GOPATH`, `SSH_AUTH_SOCK`, locale, proxies,
    /// and so on. Embedders wanting a hermetic run can override this with an
    /// explicit [`InvocationEnvironment`].
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
    /// Optional wall-clock bound on any single normal (non-persistent) unit.
    ///
    /// When set, a unit that runs longer than this is cooperatively cancelled
    /// (the same SIGTERM-and-wait teardown as `--fail-fast`/Ctrl+C) and recorded
    /// as [`UnitStatus::TimedOut`](toven_model::UnitStatus::TimedOut) — a failure
    /// — rather than being allowed to run unbounded. `None` (the default) leaves
    /// normal units unbounded. Persistent units are governed by their own
    /// [`readiness_timeout`](toven_model::ExecutionUnit::readiness_timeout) probe,
    /// not this bound.
    pub unit_timeout: Option<Duration>,
}

impl Default for ApplyOptions {
    fn default() -> Self {
        Self {
            max_parallel: std::thread::available_parallelism().map_or(1, std::num::NonZero::get),
            fail_fast: false,
            environment: InvocationEnvironment::inherit_parent(std::collections::BTreeMap::new()),
            live_output_capacity: DEFAULT_LIVE_OUTPUT_CAPACITY,
            unit_timeout: None,
        }
    }
}

/// Whether normal-unit output should stream live for this run.
///
/// Live streaming a normal unit's output is safe only when nothing else can be
/// emitting concurrently. Two things can:
/// - another unit running in parallel — excluded by requiring serial execution
///   (`max_parallel <= 1`) or a single-unit plan;
/// - a held persistent unit, which keeps running (and emitting live) across the
///   later waves that run normal units, even under serial execution — excluded
///   by requiring the plan to contain no persistent unit.
///
/// When either could interleave, normal output is buffered into a deterministic
/// per-unit block instead.
pub(super) fn stream_normal_live(options: &ApplyOptions, plan: &toven_model::Plan) -> bool {
    let serial_or_single = options.max_parallel <= 1 || plan.units.len() <= 1;
    let has_persistent = plan.units.iter().any(|unit| unit.persistent);
    serial_or_single && !has_persistent
}
