//! Run timing and cache statistics.

use std::time::{Duration, Instant};

/// Aggregated statistics for one task run.
#[derive(Debug, Clone)]
pub struct RunStats {
    started_at: Instant,
    /// Execution units included in the plan.
    pub planned_units: usize,
    /// Units skipped completely because every module was a cache hit.
    pub skipped_units: usize,
    /// Units that spawned a subprocess.
    pub subprocesses: usize,
    /// Cache decisions that were hits.
    pub cache_hits: usize,
    /// Cache decisions that were misses.
    pub cache_misses: usize,
    /// Cache decisions disabled for the invocation.
    pub cache_disabled: usize,
    /// Cache decisions forced to execute.
    pub cache_forced: usize,
    /// Completed subprocess wall time added across units.
    pub subprocess_wall: Duration,
}

impl RunStats {
    /// Create empty run statistics for a plan with `planned_units`.
    #[must_use]
    pub fn new(planned_units: usize) -> Self {
        Self {
            started_at: Instant::now(),
            planned_units,
            skipped_units: 0,
            subprocesses: 0,
            cache_hits: 0,
            cache_misses: 0,
            cache_disabled: 0,
            cache_forced: 0,
            subprocess_wall: Duration::ZERO,
        }
    }

    /// Total elapsed wall time for the run.
    #[must_use]
    pub fn total_wall(&self) -> Duration {
        self.started_at.elapsed()
    }
}
