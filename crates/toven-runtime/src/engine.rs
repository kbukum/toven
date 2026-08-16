//! [`execute`] — the wave-scheduled, bounded-parallel driver.
//!
//! Gathers a verb's shared facts once, then streams per-unit outcomes as each
//! settles: an edgeless plan runs as one wide parallel wave, an edged plan as
//! dependency-ordered waves, concurrency bounded by [`EngineConfig::jobs`] via
//! one [`rskit_worker::Pool`]. A failed unit blocks only its transitive
//! dependents; the [`RunSummary`] is derived once from the streamed outcomes.

use std::sync::Arc;

use async_trait::async_trait;
use rskit_errors::{AppError, AppResult};
use rskit_worker::{Event, Handler, Pool, PoolConfig};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::gate::{Gate, UnitState};
use crate::graph::{UnitSpec, level_waves};
use crate::lifecycle::{Progress, RunSummary, UnitReport, UnitStatus};
use crate::operation::{Completed, UnitOperation};

/// Stable pool name used in worker logs.
const POOL_NAME: &str = "toven-runtime";

/// Runtime knobs for the engine.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct EngineConfig {
    /// Maximum units executing concurrently (clamped to at least one).
    pub jobs: usize,
    /// Stop launching new waves and cancel in-flight units after the first
    /// failure. Already-running units still settle; never-launched units are
    /// reported `Cancelled`.
    pub fail_fast: bool,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            jobs: 1,
            fail_fast: false,
        }
    }
}

/// Run a [`UnitOperation`] over `units`, streaming per-unit lifecycle events to
/// `progress` and returning the aggregated [`RunSummary`].
///
/// `cancel` is a cooperative external abort (e.g. Ctrl+C): when it fires,
/// scheduling stops, in-flight units are asked to cancel, and any unit that
/// never ran settles as `Cancelled`. The worker pool is always shut down before
/// returning, even on error, so no worker task is leaked.
///
/// # Errors
/// Returns [`rskit_errors::ErrorCode::InvalidInput`] if the unit graph is malformed
/// (duplicate id, unknown dependency, or cycle), propagates a hard operation
/// error from [`UnitOperation::gather`]/[`UnitOperation::run`], and propagates
/// pool submission/shutdown failures. Ordinary unit failures are recorded in the
/// returned summary, not as `Err`.
pub async fn execute<Op: UnitOperation>(
    units: &[UnitSpec],
    operation: Op,
    config: EngineConfig,
    progress: &mut dyn Progress<Op::Outcome>,
    cancel: CancellationToken,
) -> AppResult<RunSummary> {
    // Validate and level the graph before any I/O so a malformed plan fails fast.
    let waves = level_waves(units)?;
    let gate = Gate::new(units);

    let operation = Arc::new(operation);
    // The single shared GATHER: resolved once, before any per-unit work.
    let shared = Arc::new(operation.gather().await?);
    let handler = Arc::new(OpHandler {
        operation: Arc::clone(&operation),
        shared: Arc::clone(&shared),
    });
    let pool = Pool::new(
        handler,
        PoolConfig::new(POOL_NAME)
            .with_size(config.jobs.max(1))
            .with_queue_size(units.len().max(1)),
    );

    // Drive the waves, then shut the pool down unconditionally so a driver error
    // still releases worker tasks. The first error (driver, then shutdown) is
    // returned after cleanup.
    let driven = drive::<Op>(&pool, units, &waves, gate, &config, progress, &cancel).await;
    let shutdown = pool.shutdown().await;
    let summary = driven?;
    shutdown?;
    Ok(summary)
}

/// Worker-pool handler that runs one unit's operation against the shared value.
struct OpHandler<Op: UnitOperation> {
    operation: Arc<Op>,
    shared: Arc<Op::Shared>,
}

#[async_trait]
impl<Op: UnitOperation> Handler<String, Completed<Op::Outcome>> for OpHandler<Op> {
    async fn handle(
        &self,
        unit_id: String,
        _emit: mpsc::Sender<Event<Completed<Op::Outcome>>>,
        cancel: CancellationToken,
    ) -> AppResult<Completed<Op::Outcome>> {
        self.operation.run(&self.shared, &unit_id, cancel).await
    }
}

/// Wave loop: submit each wave's pending units, stream their settled outcomes,
/// gate dependents on failure, then sweep never-run units as `Cancelled`.
async fn drive<Op: UnitOperation>(
    pool: &Pool<String, Completed<Op::Outcome>>,
    units: &[UnitSpec],
    waves: &[Vec<String>],
    mut gate: Gate,
    config: &EngineConfig,
    progress: &mut dyn Progress<Op::Outcome>,
    cancel: &CancellationToken,
) -> AppResult<RunSummary> {
    let mut summary = RunSummary::new(units.len());
    let mut aborting = false;

    for wave in waves {
        if cancel.is_cancelled() || (config.fail_fast && summary.has_failures()) {
            break;
        }

        // Submit every pending unit in the wave; the pool semaphore bounds how many
        // actually run at once. Blocked/settled units (from an earlier failure) are skipped.
        let mut joins: JoinSet<(String, AppResult<Completed<Op::Outcome>>)> = JoinSet::new();
        let mut inflight = Vec::<CancellationToken>::new();
        for unit_id in wave {
            if !matches!(gate.state(unit_id), UnitState::Pending) {
                continue;
            }
            let handle = pool.submit(unit_id.clone()).await?;
            progress.started(unit_id)?;
            inflight.push(handle.cancel_token());
            let id = unit_id.clone();
            joins.spawn(async move { (id, handle.result().await) });
        }

        while !joins.is_empty() {
            let joined = tokio::select! {
                // External abort: stop selecting it once handled (guard against a busy spin),
                // ask in-flight units to cancel, then keep draining their results below.
                () = cancel.cancelled(), if !aborting => {
                    aborting = true;
                    for token in &inflight {
                        token.cancel();
                    }
                    continue;
                }
                joined = joins.join_next() => joined,
            };
            let Some(joined) = joined else {
                break;
            };
            let (unit_id, result) = joined.map_err(AppError::internal)?;
            match result {
                Ok(completed) => {
                    let failed = completed.failed;
                    settle(&mut summary, &mut gate, progress, &unit_id, completed)?;
                    if failed && config.fail_fast && !aborting {
                        aborting = true;
                        for token in &inflight {
                            token.cancel();
                        }
                    }
                }
                // A cancelled worker leaves its unit `Pending`; the post-wave sweep emits its
                // terminal `Cancelled` event, so dropping the cancellation error is safe.
                Err(_) if aborting => {}
                // A hard operation error aborts the run: tear down the concurrent siblings
                // (as fail-fast/external-cancel do) so we don't wait out the pool grace
                // period, then propagate the typed error.
                Err(error) => {
                    for token in &inflight {
                        token.cancel();
                    }
                    return Err(error);
                }
            }
        }
    }

    cancel_unscheduled(units, &gate, &mut summary, progress)?;
    Ok(summary)
}

/// Emit a ran unit's settled outcome, then — on failure — stream a terminal
/// `Blocked` event for each transitive dependent it gates.
fn settle<T: Clone>(
    summary: &mut RunSummary,
    gate: &mut Gate,
    progress: &mut dyn Progress<T>,
    unit_id: &str,
    completed: Completed<T>,
) -> AppResult<()> {
    let status = if completed.failed {
        summary.failed += 1;
        UnitStatus::Failed
    } else {
        summary.succeeded += 1;
        gate.satisfy(unit_id);
        UnitStatus::Succeeded
    };
    progress.settled(&UnitReport {
        unit_id: unit_id.to_string(),
        status,
        outcome: Some(completed.outcome),
    })?;

    if completed.failed {
        for blocked in gate.fail_and_block_dependents(unit_id) {
            summary.blocked += 1;
            progress.settled(&UnitReport {
                unit_id: blocked,
                status: UnitStatus::Blocked,
                outcome: None,
            })?;
        }
    }
    Ok(())
}

/// Sweep every unit still `Pending` (never launched because an abort stopped
/// scheduling, or interrupted in flight) and stream a terminal `Cancelled`
/// event, in plan order for determinism.
fn cancel_unscheduled<T>(
    units: &[UnitSpec],
    gate: &Gate,
    summary: &mut RunSummary,
    progress: &mut dyn Progress<T>,
) -> AppResult<()> {
    for unit in units {
        if !matches!(gate.state(&unit.id), UnitState::Pending) {
            continue;
        }
        summary.cancelled += 1;
        progress.settled(&UnitReport {
            unit_id: unit.id.clone(),
            status: UnitStatus::Cancelled,
            outcome: None,
        })?;
    }
    Ok(())
}
