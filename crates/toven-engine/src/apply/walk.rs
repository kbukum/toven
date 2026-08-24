//! Wave walk: cache-hit skip, grouped execution, failure gating, held teardown.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use rskit_errors::{AppError, AppResult, ErrorCode};
use tokio::sync::mpsc::{Receiver, error::TryRecvError};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use toven_model::{CacheVerdict, Event, Plan, RunStats, UnitStatus};
use toven_ports::{CacheWriter, RawOutputSink, Reporter};

use crate::output::{OutputMode, UnitOutputChannel};

use super::ApplyOptions;
use super::budget::BudgetPlan;
use super::gating::{Gate, UnitState, unit_index};
use super::persistent::held::HeldSet;
use super::pool::{ApplyPool, WorkItem, WorkOutcome};
use super::record::record_success;

pub(super) struct Walker<'a, S: RawOutputSink> {
    plan: &'a Plan,
    cache: &'a dyn CacheWriter,
    reporter: &'a mut dyn Reporter,
    output: &'a mut UnitOutputChannel<S>,
    options: ApplyOptions,
    units: BTreeMap<String, toven_model::ExecutionUnit>,
    gate: Gate,
    held: HeldSet,
    stats: RunStats,
    dropped_output: Arc<AtomicUsize>,
    /// Stream normal-unit output live instead of buffering a per-unit block.
    /// Set when the sink de-interleaves concurrent output, or when no two units
    /// can run concurrently (see [`super::entry::apply`]).
    stream_normal_live: bool,
    /// Whether the sink renders one live region per unit, so each executed unit
    /// is wrapped in a `begin_unit`/`end_unit` lifecycle.
    concurrent_live: bool,
    /// Executed units that opened a sink region and still owe an `end_unit`, so
    /// the lifecycle is perfectly paired regardless of which terminal path a
    /// unit takes.
    regioned: std::collections::HashSet<String>,
    /// Cooperative external cancellation (Ctrl+C); fires the fail-fast
    /// teardown.
    external_cancel: CancellationToken,
    /// Resolved compute-budget policy: the total thread budget and the
    /// per-ecosystem env names its share is injected through.
    budget: BudgetPlan,
    /// Number of units the current wave runs concurrently (`min(max_parallel,
    /// group count)`), the deterministic divisor for the per-unit budget share.
    wave_parallel: usize,
}

impl<'a, S: RawOutputSink> Walker<'a, S> {
    pub(super) fn new(
        plan: &'a Plan,
        cache: &'a dyn CacheWriter,
        reporter: &'a mut dyn Reporter,
        output: &'a mut UnitOutputChannel<S>,
        options: ApplyOptions,
        dropped_output: Arc<AtomicUsize>,
        external_cancel: CancellationToken,
    ) -> Self {
        let concurrent_live = output.supports_concurrent_live();
        let stream_normal_live =
            super::options::stream_normal_live(&options, plan, concurrent_live);
        let budget = BudgetPlan::new(
            options.compute_budget,
            options.ecosystem_budgets.clone(),
            options.budget_env.clone(),
        );
        let mut stats = RunStats::new(plan.units.len());
        for unit in &plan.units {
            match unit.cache {
                CacheVerdict::Hit => stats.cache_hits += 1,
                CacheVerdict::Miss => stats.cache_misses += 1,
                CacheVerdict::Disabled => stats.cache_disabled += 1,
                CacheVerdict::Forced => stats.cache_forced += 1,
            }
        }
        Self {
            plan,
            cache,
            reporter,
            output,
            options,
            units: unit_index(plan),
            gate: super::gating::gate_for(plan),
            held: HeldSet::new(plan),
            stats,
            dropped_output,
            stream_normal_live,
            concurrent_live,
            regioned: std::collections::HashSet::new(),
            external_cancel,
            budget,
            wave_parallel: 1,
        }
    }

    pub(super) async fn run(
        mut self,
        pool: ApplyPool,
        mut live_output: Receiver<toven_model::UnitOutput>,
    ) -> AppResult<RunStats> {
        let start = Instant::now();
        for unit in &self.plan.units {
            // Persistent units always stream live. Normal units stream live only when
            // nothing else can emit concurrently — serial or single-unit execution, and no
            // held persistent unit in the plan; otherwise their output is buffered into a
            // deterministic per-unit block.
            let mode = if unit.persistent || self.stream_normal_live {
                OutputMode::Live
            } else {
                OutputMode::Buffered
            };
            self.output.register(unit.id.clone(), mode);
        }

        // Run the wave schedule, then tear down held processes and shut the pool down
        // unconditionally: an error from any wave must still release persistent child
        // processes and worker tasks instead of leaking them. The first failure (waves,
        // then teardown, then shutdown) is returned to the caller after cleanup has
        // run.
        let wave_result = self.run_waves(&pool, &mut live_output).await;
        let teardown_result = self.teardown_held(&mut live_output).await;
        let shutdown_result = pool.shutdown().await;

        wave_result?;
        let torn_down = teardown_result?;
        shutdown_result?;

        for unit_id in torn_down {
            // Persistent units stream live, so there is no buffered block to flush; calling
            // `finish` would only clear the unit's Live mode and risk dropping late chunks.
            // The live channel is fully drained above (and again on drop), so emit the
            // terminal event without finishing.
            self.finish_unit_event(&unit_id, UnitStatus::TornDown)?;
        }
        self.cancel_unscheduled()?;
        self.stats.duration_ms = Some(start.elapsed().as_millis().try_into().unwrap_or(u64::MAX));
        self.stats.dropped_output_chunks = self.dropped_output.load(Ordering::Relaxed);
        // Let the output sink emit any end-of-run epilogue (e.g. a consolidated failure
        // section) after the live area has drained but before the run summary, so
        // failing units land directly above the summary.
        self.output.finish_run()?;
        self.reporter.emit(&Event::RunFinished {
            summary: self.stats,
        })?;
        Ok(self.stats)
    }

    async fn run_waves(
        &mut self,
        pool: &ApplyPool,
        live_output: &mut Receiver<toven_model::UnitOutput>,
    ) -> AppResult<()> {
        for wave in &self.plan.waves {
            // Stop launching new waves once fail-fast tripped or Ctrl+C fired; the post-run
            // `cancel_unscheduled` sweep emits the terminal `Cancelled` events for whatever
            // stayed `Pending`.
            if self.external_cancel.is_cancelled()
                || (self.options.fail_fast && self.stats.has_failures())
            {
                break;
            }
            self.run_wave(wave, pool, live_output).await?;
        }
        self.drain_live_output(live_output)?;
        Ok(())
    }

    /// Emit a terminal `Cancelled` event for every planned unit that never
    /// reached a terminal state.
    ///
    /// Fail-fast can abort the run before later waves are scheduled, and an
    /// in-flight unit's worker can be cancelled mid-run; in both cases the unit
    /// stays `Pending` in the gate. Without this sweep those units would carry
    /// no terminal `UnitFinished` event, leaving the event stream short of
    /// `planned_units` and indistinguishable from units that were never in the
    /// plan. Plan order is preserved for deterministic output.
    fn cancel_unscheduled(&mut self) -> AppResult<()> {
        let plan = self.plan;
        for unit in &plan.units {
            if !matches!(self.gate.state(&unit.id), UnitState::Pending) {
                continue;
            }
            // Buffered units finish to flush any captured block and release the per-unit
            // channel state now. Persistent units stream live, so finishing would clear
            // their Live mode and risk dropping late chunks; the live channel is already
            // drained, so only the terminal event is emitted.
            if !unit.persistent {
                self.output.finish(&unit.id)?;
            }
            self.stats.cancelled_units += 1;
            let unit_id = unit.id.clone();
            self.finish_unit_event(&unit_id, UnitStatus::Cancelled)?;
        }
        Ok(())
    }

    /// Tear down every held persistent process while draining live output
    /// concurrently. If output sink pushes fail, drops the remaining output and
    /// still completes teardown before returning the first output error.
    ///
    /// Draining during teardown is required for correctness, not just latency:
    /// a reader thread blocked on a full bounded bridge would otherwise never
    /// finish, so the process it feeds could never join and `teardown_all`
    /// would deadlock. Borrowing `held` and `output` as disjoint fields lets
    /// the teardown future and the drain push run together.
    async fn teardown_held(
        &mut self,
        live_output: &mut Receiver<toven_model::UnitOutput>,
    ) -> AppResult<Vec<String>> {
        let held = &mut self.held;
        let output = &mut *self.output;
        let teardown = held.teardown_all();
        tokio::pin!(teardown);
        let mut output_error: Option<rskit_errors::AppError> = None;
        loop {
            tokio::select! {
                result = &mut teardown => {
                    let torn_down = result?;
                    // After teardown completes, drain any remaining buffered output, dropping chunks if the sink fails (best-effort).
                    while let Ok(chunk) = live_output.try_recv() {
                        if output_error.is_none() && let Err(e) = output.push(chunk) {
                            output_error = Some(e);
                        }
                    }
                    // Return teardown success, but re-raise any output error that occurred.
                    if let Some(err) = output_error {
                        return Err(err);
                    }
                    return Ok(torn_down);
                }
                Some(chunk) = live_output.recv() => {
                    // If output is already failing, drop remaining chunks silently.
                    if output_error.is_none() && let Err(e) = output.push(chunk) {
                        output_error = Some(e);
                    }
                }
            }
        }
    }

    async fn run_wave(
        &mut self,
        wave: &[String],
        pool: &ApplyPool,
        live_output: &mut Receiver<toven_model::UnitOutput>,
    ) -> AppResult<()> {
        let mut groups = BTreeMap::<String, VecDeque<String>>::new();
        for unit_id in wave {
            if !matches!(self.gate.state(unit_id), UnitState::Pending) {
                continue;
            }
            let unit = self.unit(unit_id)?;
            if unit.cache.is_hit() {
                self.cached(unit_id, live_output).await?;
                continue;
            }
            let group = unit
                .resource_group
                .clone()
                .unwrap_or_else(|| format!("unit:{unit_id}"));
            groups.entry(group).or_default().push_back(unit_id.clone());
        }
        // Each resource group runs one unit at a time, so the wave's concurrency
        // is its group count, capped by the pool. This is the deterministic
        // divisor for the compute-budget share (derived from plan structure, not
        // live scheduling), so plans and goldens stay stable.
        self.wave_parallel = self.options.max_parallel.min(groups.len()).max(1);
        self.run_groups(groups, pool, live_output).await
    }

    async fn run_groups(
        &mut self,
        groups: BTreeMap<String, VecDeque<String>>,
        pool: &ApplyPool,
        live_output: &mut Receiver<toven_model::UnitOutput>,
    ) -> AppResult<()> {
        // Flatten to indexed queues, preserving the deterministic group order; the
        // index is the group's stable identity for compute-budget attribution and
        // for resubmitting the next unit within the same group.
        let mut groups: Vec<VecDeque<String>> = groups.into_values().collect();
        let mut joins = JoinSet::new();
        let mut cancels = Vec::<CancellationToken>::new();
        // Keep at most `wave_parallel` units in flight rather than submitting every
        // group up front: over-submitting lets the pool start a later unit the instant
        // an earlier one's worker returns, so its live output can be drained ahead of
        // the earlier unit's terminal event and corrupt the single linear stream (the
        // `--jobs 1` byte-stable ordering). `next_group` is the cursor over not-yet-
        // started groups; a group is (re)activated only as an in-flight slot frees.
        let mut next_group = 0;
        while next_group < groups.len() && cancels.len() < self.wave_parallel {
            self.submit_next(
                next_group,
                &mut groups[next_group],
                pool,
                &mut joins,
                &mut cancels,
            )
            .await?;
            next_group += 1;
        }

        let mut teardown_cancelled = false;
        // A clone so the cancel future can be awaited in `select!` without borrowing
        // `self` (the other branch needs `&mut self`).
        let external_cancel = self.external_cancel.clone();
        while !joins.is_empty() {
            let joined = tokio::select! {
                // Ctrl+C (or any external cancel): fire the same teardown a fail-fast failure does — stop scheduling, SIGTERM every in-flight worker, then fall through to held teardown / pool shutdown so no child process is abandoned. Guarded so the already-ready cancelled future cannot busy-spin the loop.
                () = external_cancel.cancelled(), if !teardown_cancelled => {
                    pool.close();
                    teardown_cancelled = true;
                    for cancel in &cancels {
                        cancel.cancel();
                    }
                    continue;
                }
                Some(chunk) = live_output.recv() => {
                    self.output.push(chunk)?;
                    continue;
                }
                joined = joins.join_next() => joined,
            };
            let Some(joined) = joined else {
                break;
            };
            let (group_index, unit_id, result) = joined.map_err(AppError::internal)?;
            let failed = match result {
                Ok(outcome) => self.finish_result(&unit_id, outcome, live_output).await?,
                // A cancelled worker leaves its unit `Pending`; the post-wave `cancel_unscheduled`
                // sweep emits its terminal `Cancelled` event and finishes its channel, so dropping
                // the error here is safe rather than lossy.
                Err(_error) if teardown_cancelled => continue,
                Err(error) => return Err(error),
            };
            if failed && self.options.fail_fast {
                pool.close();
                teardown_cancelled = true;
                for cancel in &cancels {
                    cancel.cancel();
                }
            }
            if !(teardown_cancelled || failed && self.options.fail_fast) {
                // The finished unit freed one in-flight slot. Prefer the next unit in the
                // same resource group (groups run one unit at a time); once that group is
                // exhausted, advance the cursor to activate the next not-yet-started group.
                // This keeps in-flight units at `wave_parallel` without over-submitting.
                if groups[group_index].is_empty() {
                    while next_group < groups.len() {
                        let index = next_group;
                        next_group += 1;
                        if !groups[index].is_empty() {
                            self.submit_next(
                                index,
                                &mut groups[index],
                                pool,
                                &mut joins,
                                &mut cancels,
                            )
                            .await?;
                            break;
                        }
                    }
                } else {
                    self.submit_next(
                        group_index,
                        &mut groups[group_index],
                        pool,
                        &mut joins,
                        &mut cancels,
                    )
                    .await?;
                }
            }
        }
        self.drain_live_output(live_output)?;
        Ok(())
    }

    async fn submit_next(
        &mut self,
        group_index: usize,
        group: &mut VecDeque<String>,
        pool: &ApplyPool,
        joins: &mut JoinSet<(usize, String, AppResult<WorkOutcome>)>,
        cancels: &mut Vec<CancellationToken>,
    ) -> AppResult<()> {
        let Some(unit_id) = group.pop_front() else {
            return Ok(());
        };
        let unit = self.unit(&unit_id)?.clone();
        let extra_env = if self.budget.is_active() {
            let scope = toven_model::EcosystemScope::new(
                unit.module.member().cloned(),
                unit.module.module().ecosystem.clone(),
            );
            self.budget.env_for(&scope, self.wave_parallel)
        } else {
            std::collections::BTreeMap::new()
        };
        let handle = pool.submit(WorkItem::new(unit, extra_env)).await?;
        self.reporter.emit(&Event::UnitStarted {
            unit_id: unit_id.clone(),
        })?;
        self.begin_region(&unit_id)?;
        cancels.push(handle.cancel_token());
        joins.spawn(async move {
            let result = handle.result().await;
            (group_index, unit_id, result)
        });
        Ok(())
    }

    async fn finish_result(
        &mut self,
        unit_id: &str,
        result: WorkOutcome,
        live_output: &mut Receiver<toven_model::UnitOutput>,
    ) -> AppResult<bool> {
        match result {
            WorkOutcome::Normal {
                success,
                exit_code,
                output,
            } => {
                if self.stream_normal_live {
                    // Output streamed live through the observer bridge; the returned `output` is
                    // empty. Drain any chunks still queued (the runner returns only after its
                    // reader threads joined, so none can arrive later) before finishing, so
                    // finishing — which clears the unit's Live mode — cannot strand a chunk.
                    self.drain_live_output(live_output)?;
                    self.output.finish(unit_id)?;
                } else {
                    self.route_output(unit_id, output)?;
                }
                if success {
                    self.succeeded(unit_id, live_output).await?;
                    Ok(false)
                } else {
                    self.failed(unit_id, UnitStatus::Failed, exit_code, live_output)
                        .await?;
                    Ok(true)
                }
            }
            WorkOutcome::TimedOut { output } => {
                // Same output routing as a normal unit (live-drain or buffered block), then a
                // distinct timeout failure so the summary and exit reflect it as its own reason
                // rather than a plain non-zero exit.
                if self.stream_normal_live {
                    self.drain_live_output(live_output)?;
                    self.output.finish(unit_id)?;
                } else {
                    self.route_output(unit_id, output)?;
                }
                self.failed(unit_id, UnitStatus::TimedOut, None, live_output)
                    .await?;
                Ok(true)
            }
            WorkOutcome::PersistentReady { output, process } => {
                self.route_output_chunks(output)?;
                self.held.hold(process);
                self.reporter.emit(&Event::UnitReady {
                    unit_id: unit_id.to_string(),
                })?;
                self.stats.ran_units += 1;
                self.gate.satisfy(unit_id);
                self.drain_dependents(unit_id, live_output).await?;
                Ok(false)
            }
            WorkOutcome::FailedReadiness { output } => {
                // Persistent units stream live; routing the synthetic readiness error as a live
                // chunk (without `finish`) keeps the unit's Live mode registered so any late
                // chunks the failed process already emitted are still flushed by a later
                // `drain_live_output` instead of being dropped as a fresh unregistered unit.
                self.route_output_chunks(output)?;
                self.failed(unit_id, UnitStatus::FailedReadiness, None, live_output)
                    .await?;
                Ok(true)
            }
        }
    }

    async fn cached(
        &mut self,
        unit_id: &str,
        live_output: &mut Receiver<toven_model::UnitOutput>,
    ) -> AppResult<()> {
        self.stats.cached_units += 1;
        self.gate.satisfy(unit_id);
        // A cache hit produces no output, but the unit was registered with a channel
        // mode at run start; finish it so its per-unit channel state is released now
        // rather than lingering until the channel is dropped.
        self.output.finish(unit_id)?;
        self.finish_unit_event(unit_id, UnitStatus::Cached)?;
        self.drain_dependents(unit_id, live_output).await
    }

    async fn succeeded(
        &mut self,
        unit_id: &str,
        live_output: &mut Receiver<toven_model::UnitOutput>,
    ) -> AppResult<()> {
        let unit = self.unit(unit_id)?;
        record_success(unit, self.cache)?;
        self.stats.ran_units += 1;
        self.gate.satisfy(unit_id);
        self.finish_unit_event(unit_id, UnitStatus::Succeeded)?;
        self.drain_dependents(unit_id, live_output).await
    }

    async fn failed(
        &mut self,
        unit_id: &str,
        status: UnitStatus,
        exit_code: Option<i32>,
        live_output: &mut Receiver<toven_model::UnitOutput>,
    ) -> AppResult<()> {
        match status {
            UnitStatus::FailedReadiness => self.stats.failed_readiness_units += 1,
            UnitStatus::Failed => self.stats.failed_units += 1,
            UnitStatus::TimedOut => self.stats.timed_out_units += 1,
            _ => {}
        }
        self.emit_finish(unit_id, status, exit_code)?;
        self.drain_dependents(unit_id, live_output).await?;
        for blocked in self.gate.fail_and_block_dependents(unit_id) {
            self.stats.blocked_units += 1;
            self.finish_unit_event(&blocked, UnitStatus::Blocked)?;
            self.drain_dependents(&blocked, live_output).await?;
        }
        Ok(())
    }

    /// Tear down any held persistent units whose dependents just drained, while
    /// continuing to drain live output so a process parked on a full bounded
    /// bridge can make progress and join.
    async fn drain_dependents(
        &mut self,
        unit_id: &str,
        live_output: &mut Receiver<toven_model::UnitOutput>,
    ) -> AppResult<()> {
        for held_id in self.held.dependent_finished(unit_id) {
            if self.teardown_held_unit(&held_id, live_output).await? {
                // Held units are persistent (live mode): no buffered block to flush, and
                // finishing would clear their mode and risk dropping late chunks. Leave the
                // unit live so any final output drains through `drain_live_output`; only emit
                // the terminal event.
                self.finish_unit_event(&held_id, UnitStatus::TornDown)?;
            }
        }
        Ok(())
    }

    /// Shut down one held unit, draining live output concurrently with the
    /// blocking process shutdown. The shutdown runs on a blocking thread (via
    /// [`SharedHeldProcess::shutdown_offloaded`]); this loop keeps pulling from
    /// the bounded live-output bridge so a reader thread parked in
    /// `blocking_send` can drain and the process can join instead of
    /// deadlocking. Returns whether a held unit was present and torn down.
    async fn teardown_held_unit(
        &mut self,
        held_id: &str,
        live_output: &mut Receiver<toven_model::UnitOutput>,
    ) -> AppResult<bool> {
        let Some(process) = self.held.take_for_teardown(held_id) else {
            return Ok(false);
        };
        let output = &mut *self.output;
        let shutdown = process.shutdown_offloaded();
        tokio::pin!(shutdown);
        let mut output_error: Option<rskit_errors::AppError> = None;
        loop {
            tokio::select! {
                result = &mut shutdown => {
                    result?;
                    // After shutdown completes, drain whatever the process flushed last, dropping chunks only if the sink itself already failed (best-effort, never silent loss).
                    while let Ok(chunk) = live_output.try_recv() {
                        if output_error.is_none() && let Err(e) = output.push(chunk) {
                            output_error = Some(e);
                        }
                    }
                    if let Some(err) = output_error {
                        return Err(err);
                    }
                    return Ok(true);
                }
                Some(chunk) = live_output.recv() => {
                    if output_error.is_none() && let Err(e) = output.push(chunk) {
                        output_error = Some(e);
                    }
                }
            }
        }
    }

    /// Open a sink region for an executed unit when the sink renders one region
    /// per unit, recording it so the matching `end_unit` is emitted exactly
    /// once.
    fn begin_region(&mut self, unit_id: &str) -> AppResult<()> {
        if self.concurrent_live && !self.regioned.contains(unit_id) {
            self.output.begin_unit(unit_id, unit_id)?;
            self.regioned.insert(unit_id.to_string());
        }
        Ok(())
    }

    /// Emit a unit's terminal event, first collapsing its sink region (if it
    /// opened one) so `begin_unit`/`end_unit` stay perfectly paired across
    /// every terminal path (success, failure, timeout, blocked, cancelled, torn
    /// down).
    fn finish_unit_event(&mut self, unit_id: &str, status: UnitStatus) -> AppResult<()> {
        self.emit_finish(unit_id, status, None)
    }

    fn emit_finish(
        &mut self,
        unit_id: &str,
        status: UnitStatus,
        exit_code: Option<i32>,
    ) -> AppResult<()> {
        if self.regioned.remove(unit_id) {
            self.output.end_unit(unit_id, status)?;
        }
        self.reporter.emit(&Event::UnitFinished {
            unit_id: unit_id.to_string(),
            status,
            exit_code,
        })
    }

    fn route_output(
        &mut self,
        unit_id: &str,
        chunks: Vec<toven_model::UnitOutput>,
    ) -> AppResult<()> {
        self.route_output_chunks(chunks)?;
        self.output.finish(unit_id)
    }

    fn route_output_chunks(&mut self, chunks: Vec<toven_model::UnitOutput>) -> AppResult<()> {
        for chunk in chunks {
            self.output.push(chunk)?;
        }
        Ok(())
    }

    fn drain_live_output(
        &mut self,
        live_output: &mut Receiver<toven_model::UnitOutput>,
    ) -> AppResult<()> {
        loop {
            match live_output.try_recv() {
                Ok(chunk) => self.output.push(chunk)?,
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return Ok(()),
            }
        }
    }

    fn unit(&self, unit_id: &str) -> AppResult<&toven_model::ExecutionUnit> {
        self.units.get(unit_id).ok_or_else(|| {
            AppError::new(
                ErrorCode::Internal,
                format!("plan wave references unknown unit '{unit_id}'"),
            )
        })
    }
}
