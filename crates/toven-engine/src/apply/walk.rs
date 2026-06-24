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
}

impl<'a, S: RawOutputSink> Walker<'a, S> {
    pub(super) fn new(
        plan: &'a Plan,
        cache: &'a dyn CacheWriter,
        reporter: &'a mut dyn Reporter,
        output: &'a mut UnitOutputChannel<S>,
        options: ApplyOptions,
        dropped_output: Arc<AtomicUsize>,
    ) -> Self {
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
            gate: Gate::new(plan),
            held: HeldSet::new(plan),
            stats,
            dropped_output,
        }
    }

    pub(super) async fn run(
        mut self,
        pool: ApplyPool,
        mut live_output: Receiver<toven_model::UnitOutput>,
    ) -> AppResult<RunStats> {
        let start = Instant::now();
        for unit in &self.plan.units {
            self.output.register(
                unit.id.clone(),
                if unit.persistent {
                    OutputMode::Live
                } else {
                    OutputMode::Buffered
                },
            );
        }

        // Run the wave schedule, then tear down held processes and shut the pool
        // down unconditionally: an error from any wave must still release
        // persistent child processes and worker tasks instead of leaking them.
        // The first failure (waves, then teardown, then shutdown) is returned to
        // the caller after cleanup has run.
        let wave_result = self.run_waves(&pool, &mut live_output).await;
        let teardown_result = self.teardown_held(&mut live_output).await;
        let shutdown_result = pool.shutdown().await;

        wave_result?;
        let torn_down = teardown_result?;
        shutdown_result?;

        for unit_id in torn_down {
            // Persistent units stream live, so there is no buffered block to
            // flush; calling `finish` would only clear the unit's Live mode and
            // risk dropping late chunks. The live channel is fully drained above
            // (and again on drop), so emit the terminal event without finishing.
            self.reporter.emit(&Event::UnitFinished {
                unit_id,
                status: UnitStatus::TornDown,
            })?;
        }
        self.stats.duration_ms = Some(start.elapsed().as_millis().try_into().unwrap_or(u64::MAX));
        self.stats.dropped_output_chunks = self.dropped_output.load(Ordering::Relaxed);
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
            if self.options.fail_fast && self.stats.has_failures() {
                break;
            }
            self.run_wave(wave, pool, live_output).await?;
        }
        self.drain_live_output(live_output)?;
        Ok(())
    }

    /// Tear down every held persistent process while draining live output
    /// concurrently.
    ///
    /// Draining during teardown is required for correctness, not just latency: a
    /// reader thread blocked on a full bounded bridge would otherwise never
    /// finish, so the process it feeds could never join and `teardown_all` would
    /// deadlock. Borrowing `held` and `output` as disjoint fields lets the
    /// teardown future and the drain push run together.
    async fn teardown_held(
        &mut self,
        live_output: &mut Receiver<toven_model::UnitOutput>,
    ) -> AppResult<Vec<String>> {
        let held = &mut self.held;
        let output = &mut *self.output;
        let teardown = held.teardown_all();
        tokio::pin!(teardown);
        loop {
            tokio::select! {
                result = &mut teardown => {
                    let torn_down = result?;
                    while let Ok(chunk) = live_output.try_recv() {
                        output.push(chunk)?;
                    }
                    return Ok(torn_down);
                }
                Some(chunk) = live_output.recv() => {
                    output.push(chunk)?;
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
                self.cached(unit_id)?;
                continue;
            }
            let group = unit
                .resource_group
                .clone()
                .unwrap_or_else(|| format!("unit:{unit_id}"));
            groups.entry(group).or_default().push_back(unit_id.clone());
        }
        self.run_groups(groups, pool, live_output).await
    }

    async fn run_groups(
        &mut self,
        mut groups: BTreeMap<String, VecDeque<String>>,
        pool: &ApplyPool,
        live_output: &mut Receiver<toven_model::UnitOutput>,
    ) -> AppResult<()> {
        let mut joins = JoinSet::new();
        let mut cancels = Vec::<CancellationToken>::new();
        for (index, group) in groups.values_mut().enumerate() {
            self.submit_next(index, group, pool, &mut joins, &mut cancels)
                .await?;
        }

        let mut fail_fast_cancelled = false;
        while !joins.is_empty() {
            let joined = tokio::select! {
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
                Ok(outcome) => self.finish_result(&unit_id, outcome)?,
                Err(_error) if fail_fast_cancelled => continue,
                Err(error) => return Err(error),
            };
            if failed && self.options.fail_fast {
                pool.close();
                fail_fast_cancelled = true;
                for cancel in &cancels {
                    cancel.cancel();
                }
            }
            if !(fail_fast_cancelled || failed && self.options.fail_fast)
                && let Some(group) = groups.values_mut().nth(group_index)
            {
                self.submit_next(group_index, group, pool, &mut joins, &mut cancels)
                    .await?;
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
        self.reporter.emit(&Event::UnitStarted {
            unit_id: unit_id.clone(),
        })?;
        let handle = pool.submit(WorkItem::new(unit)).await?;
        cancels.push(handle.cancel_token());
        joins.spawn(async move {
            let result = handle.result().await;
            (group_index, unit_id, result)
        });
        Ok(())
    }

    fn finish_result(&mut self, unit_id: &str, result: WorkOutcome) -> AppResult<bool> {
        match result {
            WorkOutcome::Normal { success, output } => {
                self.route_output(unit_id, output)?;
                if success {
                    self.succeeded(unit_id)?;
                    Ok(false)
                } else {
                    self.failed(unit_id, UnitStatus::Failed)?;
                    Ok(true)
                }
            }
            WorkOutcome::PersistentReady { output, process } => {
                self.route_output_chunks(output)?;
                self.held.hold(process);
                self.reporter.emit(&Event::UnitReady {
                    unit_id: unit_id.to_string(),
                })?;
                self.stats.ran_units += 1;
                self.gate.satisfy(unit_id);
                self.drain_dependents(unit_id)?;
                Ok(false)
            }
            WorkOutcome::FailedReadiness { output } => {
                // Persistent units stream live; routing the synthetic readiness
                // error as a live chunk (without `finish`) keeps the unit's Live
                // mode registered so any late chunks the failed process already
                // emitted are still flushed by a later `drain_live_output`
                // instead of being dropped as a fresh unregistered unit.
                self.route_output_chunks(output)?;
                self.failed(unit_id, UnitStatus::FailedReadiness)?;
                Ok(true)
            }
        }
    }

    fn cached(&mut self, unit_id: &str) -> AppResult<()> {
        self.stats.cached_units += 1;
        self.gate.satisfy(unit_id);
        // A cache hit produces no output, but the unit was registered with a
        // channel mode at run start; finish it so its per-unit channel state is
        // released now rather than lingering until the channel is dropped.
        self.output.finish(unit_id)?;
        self.reporter.emit(&Event::UnitFinished {
            unit_id: unit_id.to_string(),
            status: UnitStatus::Cached,
        })?;
        self.drain_dependents(unit_id)
    }

    fn succeeded(&mut self, unit_id: &str) -> AppResult<()> {
        let unit = self.unit(unit_id)?;
        record_success(unit, self.cache)?;
        self.stats.ran_units += 1;
        self.gate.satisfy(unit_id);
        self.reporter.emit(&Event::UnitFinished {
            unit_id: unit_id.to_string(),
            status: UnitStatus::Succeeded,
        })?;
        self.drain_dependents(unit_id)
    }

    fn failed(&mut self, unit_id: &str, status: UnitStatus) -> AppResult<()> {
        match status {
            UnitStatus::FailedReadiness => self.stats.failed_readiness_units += 1,
            UnitStatus::Failed => self.stats.failed_units += 1,
            _ => {}
        }
        self.reporter.emit(&Event::UnitFinished {
            unit_id: unit_id.to_string(),
            status,
        })?;
        self.drain_dependents(unit_id)?;
        for blocked in self.gate.fail_and_block_dependents(unit_id) {
            self.stats.blocked_units += 1;
            self.reporter.emit(&Event::UnitFinished {
                unit_id: blocked.clone(),
                status: UnitStatus::Blocked,
            })?;
            self.drain_dependents(&blocked)?;
        }
        Ok(())
    }

    fn drain_dependents(&mut self, unit_id: &str) -> AppResult<()> {
        for held_id in self.held.dependent_finished(unit_id) {
            if self.held.teardown_one(&held_id)? {
                // Held units are persistent (live mode): no buffered block to
                // flush, and finishing would clear their mode and risk dropping
                // late chunks. Leave the unit live so any final output drains
                // through `drain_live_output`; only emit the terminal event.
                self.reporter.emit(&Event::UnitFinished {
                    unit_id: held_id,
                    status: UnitStatus::TornDown,
                })?;
            }
        }
        Ok(())
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
