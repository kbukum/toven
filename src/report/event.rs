//! Stable run event reporting.

use std::{
    io::Write,
    process,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::Serialize;

use crate::{
    cache::decision::{CacheDecision, CacheState},
    core::{AppError, AppResult, ExecutionUnit, Module},
    report::RunStats,
};

const SCHEMA_VERSION: u16 = 1;

/// Output format for task execution.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum OutputFormat {
    /// Human-readable terminal output.
    Human,
    /// Newline-delimited JSON events on stdout.
    Jsonl,
}

impl OutputFormat {
    /// Parse a CLI output format value.
    pub fn parse(value: &str) -> AppResult<Self> {
        match value {
            "human" => Ok(Self::Human),
            "jsonl" => Ok(Self::Jsonl),
            _ => Err(AppError::invalid_input(
                "output",
                format!("unsupported output format '{value}'"),
            )),
        }
    }
}

/// Synchronous, lossless run reporter.
pub struct RunReporter<'a, W: Write> {
    format: OutputFormat,
    stdout: &'a mut W,
    run_id: Option<String>,
    seq: u64,
    stats: RunStats,
}

impl<'a, W: Write> RunReporter<'a, W> {
    /// Create a reporter for one run.
    pub fn new(format: OutputFormat, stdout: &'a mut W, planned_units: usize) -> AppResult<Self> {
        let run_id = match format {
            OutputFormat::Human => None,
            OutputFormat::Jsonl => Some(run_id()?),
        };
        Ok(Self {
            format,
            stdout,
            run_id,
            seq: 0,
            stats: RunStats::new(planned_units),
        })
    }

    /// Return the selected output format.
    #[must_use]
    pub const fn format(&self) -> OutputFormat {
        self.format
    }

    /// Return collected run statistics.
    #[must_use]
    pub const fn stats(&self) -> &RunStats {
        &self.stats
    }

    /// Emit plan metadata.
    pub fn plan_prepared(&mut self, workspace: &str, root: &str) -> AppResult<()> {
        self.event(
            "plan.prepared",
            PlanPrepared {
                workspace,
                root,
                units: self.stats.planned_units,
            },
        )
    }

    /// Emit one cache decision.
    pub fn cache_decision(&mut self, decision: &CacheDecision) -> AppResult<()> {
        match &decision.state {
            CacheState::Hit => self.stats.cache_hits += 1,
            CacheState::Miss { .. } => self.stats.cache_misses += 1,
            CacheState::Disabled { .. } => self.stats.cache_disabled += 1,
            CacheState::Forced => self.stats.cache_forced += 1,
        }
        self.event("cache.decision", CacheDecisionEvent::from(decision))
    }

    /// Emit a cache hit that skips or coalesces a module execution.
    pub fn cache_hit(
        &mut self,
        unit: &ExecutionUnit,
        module: &Module,
        executed: bool,
    ) -> AppResult<()> {
        self.event(
            "cache.hit",
            CacheHit {
                unit_id: &unit.id,
                profile: &unit.profile,
                task: &unit.task,
                module: module.name.as_str(),
                executed,
            },
        )?;
        if self.format == OutputFormat::Human {
            if executed {
                writeln!(
                    self.stdout,
                    "cache hit (re-run as part of workspace-once): {} {}",
                    module.name, unit.task
                )
                .map_err(AppError::internal)?;
            } else {
                writeln!(self.stdout, "cache hit: {} {}", module.name, unit.task)
                    .map_err(AppError::internal)?;
            }
        }
        Ok(())
    }

    /// Record that a unit was skipped entirely.
    pub fn unit_skipped(&mut self, unit: &ExecutionUnit) -> AppResult<()> {
        self.stats.skipped_units += 1;
        self.event("unit.skipped", UnitRef::from(unit))
    }

    /// Emit a unit start.
    pub fn unit_started(&mut self, unit: &ExecutionUnit) -> AppResult<()> {
        if self.format == OutputFormat::Human {
            writeln!(self.stdout, "run: {}", unit.id).map_err(AppError::internal)?;
        }
        self.event("unit.started", UnitRef::from(unit))
    }

    /// Emit a unit finish.
    pub fn unit_finished(
        &mut self,
        unit: &ExecutionUnit,
        result: &rskit_process::ProcessResult,
        cancelled: bool,
    ) -> AppResult<()> {
        self.stats.subprocesses += 1;
        self.stats.subprocess_wall += result.duration;
        self.event(
            "unit.finished",
            UnitFinished {
                unit_id: &unit.id,
                profile: &unit.profile,
                task: &unit.task,
                success: result.success(),
                exit_code: result.exit_code,
                duration_ms: duration_ms(result.duration),
                timed_out: result.timed_out,
                cancelled,
            },
        )
    }

    /// Write child stdout without corrupting JSONL stdout.
    pub fn child_stdout<Err: Write>(&mut self, stderr: &mut Err, bytes: &[u8]) -> AppResult<()> {
        match self.format {
            OutputFormat::Human => self.stdout.write_all(bytes),
            OutputFormat::Jsonl => stderr.write_all(bytes),
        }
        .map_err(AppError::internal)
    }

    /// Emit the final successful summary.
    pub fn run_succeeded(&mut self) -> AppResult<()> {
        self.event("run.summary", RunSummary::from(self.stats()))
    }

    /// Emit the final failed summary.
    pub fn run_failed(&mut self, error: &AppError) -> AppResult<()> {
        self.event(
            "run.failed",
            RunFailed {
                code: error.code.as_str(),
                message: &error.message,
                stats: RunSummary::from(self.stats()),
            },
        )
    }

    fn event<T: Serialize>(&mut self, event: &'static str, payload: T) -> AppResult<()> {
        if self.format != OutputFormat::Jsonl {
            return Ok(());
        }
        let run_id = self.run_id.as_deref().ok_or_else(|| {
            AppError::new(
                crate::core::ErrorCode::Internal,
                "JSONL reporter was initialized without a run id",
            )
        })?;
        self.seq += 1;
        let line = JsonEvent {
            schema_version: SCHEMA_VERSION,
            seq: self.seq,
            run_id,
            event,
            payload,
        };
        serde_json::to_writer(&mut self.stdout, &line).map_err(|error| {
            AppError::new(
                crate::core::ErrorCode::Internal,
                "failed to encode JSONL event",
            )
            .with_cause(error)
        })?;
        writeln!(self.stdout).map_err(AppError::internal)
    }
}

#[derive(Serialize)]
struct JsonEvent<'a, T> {
    schema_version: u16,
    seq: u64,
    run_id: &'a str,
    event: &'static str,
    #[serde(flatten)]
    payload: T,
}

#[derive(Serialize)]
struct PlanPrepared<'a> {
    workspace: &'a str,
    root: &'a str,
    units: usize,
}

#[derive(Serialize)]
struct CacheDecisionEvent<'a> {
    profile: &'a str,
    module: &'a str,
    task: &'a str,
    state: &'static str,
    reason: Option<&'a str>,
}

impl<'a> From<&'a CacheDecision> for CacheDecisionEvent<'a> {
    fn from(decision: &'a CacheDecision) -> Self {
        let (state, reason) = match &decision.state {
            CacheState::Hit => ("hit", None),
            CacheState::Miss { reason } => ("miss", Some(reason.as_str())),
            CacheState::Disabled { reason } => ("disabled", Some(reason.as_str())),
            CacheState::Forced => ("forced", None),
        };
        Self {
            profile: &decision.profile,
            module: decision.module.as_str(),
            task: &decision.task,
            state,
            reason,
        }
    }
}

#[derive(Serialize)]
struct CacheHit<'a> {
    unit_id: &'a str,
    profile: &'a str,
    task: &'a str,
    module: &'a str,
    executed: bool,
}

#[derive(Serialize)]
struct UnitRef<'a> {
    unit_id: &'a str,
    profile: &'a str,
    task: &'a str,
    modules: Vec<&'a str>,
}

impl<'a> From<&'a ExecutionUnit> for UnitRef<'a> {
    fn from(unit: &'a ExecutionUnit) -> Self {
        Self {
            unit_id: &unit.id,
            profile: &unit.profile,
            task: &unit.task,
            modules: unit
                .modules
                .iter()
                .map(|module| module.name.as_str())
                .collect(),
        }
    }
}

#[derive(Serialize)]
struct UnitFinished<'a> {
    unit_id: &'a str,
    profile: &'a str,
    task: &'a str,
    success: bool,
    exit_code: Option<i32>,
    duration_ms: u128,
    timed_out: bool,
    cancelled: bool,
}

#[derive(Serialize)]
struct RunSummary {
    planned_units: usize,
    skipped_units: usize,
    subprocesses: usize,
    cache_hits: usize,
    cache_misses: usize,
    cache_disabled: usize,
    cache_forced: usize,
    subprocess_wall_ms: u128,
    total_wall_ms: u128,
}

impl From<&RunStats> for RunSummary {
    fn from(stats: &RunStats) -> Self {
        Self {
            planned_units: stats.planned_units,
            skipped_units: stats.skipped_units,
            subprocesses: stats.subprocesses,
            cache_hits: stats.cache_hits,
            cache_misses: stats.cache_misses,
            cache_disabled: stats.cache_disabled,
            cache_forced: stats.cache_forced,
            subprocess_wall_ms: duration_ms(stats.subprocess_wall),
            total_wall_ms: duration_ms(stats.total_wall()),
        }
    }
}

#[derive(Serialize)]
struct RunFailed<'a> {
    code: &'a str,
    message: &'a str,
    #[serde(flatten)]
    stats: RunSummary,
}

fn run_id() -> AppResult<String> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            AppError::new(
                crate::core::ErrorCode::Internal,
                "system clock is before UNIX epoch",
            )
            .with_cause(error)
        })?
        .as_nanos();
    Ok(format!("{nanos}-{}", process::id()))
}

const fn duration_ms(duration: Duration) -> u128 {
    duration.as_millis()
}
