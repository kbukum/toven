//! Aggregated run summary carried by the terminal run event.

use serde::{Deserialize, Serialize};

/// Counters summarizing one run, serialized across the driver boundary.
///
/// Pure data (no wall-clock handle) so it round-trips through serde; total wall
/// time is recorded as a resolved `duration_ms` by the reporter, not an
/// in-flight `Instant`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct RunStats {
    /// Execution units included in the plan.
    pub planned_units: usize,
    /// Units that ran a subprocess to success.
    pub ran_units: usize,
    /// Units skipped because they were cache hits.
    pub cached_units: usize,
    /// Units that failed.
    pub failed_units: usize,
    /// Units blocked by an upstream failure.
    pub blocked_units: usize,
    /// Units not run, or interrupted in flight, because the run aborted early
    /// under fail-fast (not themselves failures).
    pub cancelled_units: usize,
    /// Persistent units that never reached readiness (a failure).
    pub failed_readiness_units: usize,
    /// Units cooperatively cancelled after exceeding their per-unit execution
    /// timeout (a failure).
    pub timed_out_units: usize,
    /// Cache decisions that were hits.
    pub cache_hits: usize,
    /// Cache decisions that were misses.
    pub cache_misses: usize,
    /// Cache decisions disabled for the invocation.
    pub cache_disabled: usize,
    /// Cache decisions forced to execute.
    pub cache_forced: usize,
    /// Live persistent-output chunks dropped because the bounded output bridge
    /// was full and the producer could not block (e.g. an async-runtime
    /// producer). Zero on the blocking-backpressure path; non-zero surfaces
    /// otherwise-silent output loss.
    pub dropped_output_chunks: usize,
    /// Total wall time in milliseconds, once the run completes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

impl RunStats {
    /// Create empty statistics for a plan with `planned_units`.
    #[must_use]
    pub fn new(planned_units: usize) -> Self {
        Self {
            planned_units,
            ..Self::default()
        }
    }

    /// Whether any unit failed, was blocked, failed readiness, or timed out
    /// (drives a non-zero exit). Mirrors
    /// [`UnitStatus::is_failure`](crate::UnitStatus::is_failure).
    #[must_use]
    pub const fn has_failures(&self) -> bool {
        self.failed_units > 0
            || self.blocked_units > 0
            || self.failed_readiness_units > 0
            || self.timed_out_units > 0
    }
}
