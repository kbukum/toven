//! rskit-worker wrapper for already-computed command invocations.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rskit_errors::{AppResult, ErrorCode};
use rskit_worker::{Handler, Pool, PoolConfig, TaskHandle};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use toven_model::{ExecutionUnit, UnitOutput};
use toven_ports::{CommandRunner, Invocation, InvocationEnvironment, OutputObserver, StartOutcome};

use super::persistent::held::SharedHeldProcess;

/// Cloneable outcome produced by the worker pool.
#[derive(Clone)]
pub(super) enum WorkOutcome {
    /// A normal unit completed.
    Normal {
        /// Whether it succeeded.
        success: bool,
        /// Process exit code (`None` when the platform reported no code, e.g.
        /// signal termination). Carried so a failure can name the exit.
        exit_code: Option<i32>,
        /// Raw output chunks.
        output: Vec<UnitOutput>,
    },
    /// A normal unit exceeded its per-unit timeout and was cancelled.
    TimedOut {
        /// Raw output chunks captured before the timeout cancellation.
        output: Vec<UnitOutput>,
    },
    /// A persistent unit reached readiness.
    PersistentReady {
        /// Raw output chunks captured before readiness.
        output: Vec<UnitOutput>,
        /// Held process handle.
        process: SharedHeldProcess,
    },
    /// A persistent unit failed readiness.
    FailedReadiness {
        /// Raw output chunks captured before readiness failure.
        output: Vec<UnitOutput>,
    },
}

/// One unit submitted to the worker pool.
#[derive(Clone)]
pub(super) struct WorkItem {
    unit: ExecutionUnit,
    /// Per-unit compute-budget environment merged on top of the pool's shared
    /// environment (empty when this unit's ecosystem opts out).
    extra_env: std::collections::BTreeMap<String, String>,
}

impl WorkItem {
    /// Build a work item for `unit` with its resolved compute-budget env.
    #[must_use]
    pub(super) const fn new(
        unit: ExecutionUnit,
        extra_env: std::collections::BTreeMap<String, String>,
    ) -> Self {
        Self { unit, extra_env }
    }
}

struct WorkHandler {
    runner: Arc<dyn CommandRunner>,
    environment: InvocationEnvironment,
    output: OutputObserver,
    /// Stream normal-unit output live (through `output`) instead of capturing
    /// it for a buffered block. Only set when nothing else can emit
    /// concurrently (serial or single-unit execution and no held persistent
    /// unit), so streamed chunks never interleave.
    stream_normal_live: bool,
    /// Optional wall-clock bound on any single normal unit; on expiry the
    /// unit's own cancellation token is fired so the runner tears the child
    /// down.
    unit_timeout: Option<Duration>,
}

impl WorkHandler {
    /// The pool's shared environment with this unit's compute-budget share
    /// merged on top (the share never clobbers an operator-set base var of the
    /// same name — the base takes precedence).
    fn environment_for(&self, task: &WorkItem) -> InvocationEnvironment {
        if task.extra_env.is_empty() {
            return self.environment.clone();
        }
        let mut environment = self.environment.clone();
        for (name, value) in &task.extra_env {
            environment
                .vars
                .entry(name.clone())
                .or_insert_with(|| value.clone());
        }
        environment
    }
}

#[async_trait]
impl Handler<WorkItem, WorkOutcome> for WorkHandler {
    async fn handle(
        &self,
        task: WorkItem,
        _emit: mpsc::Sender<rskit_worker::Event<WorkOutcome>>,
        cancel: CancellationToken,
    ) -> AppResult<WorkOutcome> {
        let invocation = Invocation::from_unit(&task.unit, self.environment_for(&task));
        if task.unit.persistent {
            match self
                .runner
                .start_persistent(&invocation, cancel, self.output.clone())
                .await?
            {
                StartOutcome::Ready { output, process } => Ok(WorkOutcome::PersistentReady {
                    output,
                    process: SharedHeldProcess::new(process),
                }),
                StartOutcome::FailedReadiness { output } => {
                    Ok(WorkOutcome::FailedReadiness { output })
                }
            }
        } else {
            let live = self.stream_normal_live.then(|| self.output.clone());
            if let Some(timeout) = self.unit_timeout {
                // Bound the run: on expiry cancel this unit's own token and then await the
                // runner to completion so the child is torn down (never dropped/leaked),
                // reusing the same cooperative-cancellation path as fail-fast/Ctrl+C. `biased`
                // prefers a natural completion that lands in the same poll as the deadline.
                let run = self.runner.run(&invocation, cancel.clone(), live);
                tokio::pin!(run);
                tokio::select! {
                    biased;
                    result = &mut run => {
                        let outcome = result?;
                        Ok(WorkOutcome::Normal {
                            success: outcome.success,
                            exit_code: outcome.exit_code,
                            output: outcome.output,
                        })
                    }
                    () = tokio::time::sleep(timeout) => {
                        cancel.cancel();
                        // We deliberately cancelled this unit, so a `Cancelled` error (the fake/cooperative path) or an `Ok` killed-process outcome (the rskit-process path) both mean "timed out": salvage any captured output and report the timeout as an explicit failure verdict. Any *other* error is a genuine spawn/IO/teardown failure unrelated to our cancellation — propagate it with its cause rather than masking it as a timeout.
                        let output = match run.await {
                            Ok(outcome) => outcome.output,
                            Err(error) if error.code() == ErrorCode::Cancelled => Vec::new(),
                            Err(other) => return Err(other),
                        };
                        Ok(WorkOutcome::TimedOut { output })
                    }
                }
            } else {
                let outcome = self.runner.run(&invocation, cancel, live).await?;
                Ok(WorkOutcome::Normal {
                    success: outcome.success,
                    exit_code: outcome.exit_code,
                    output: outcome.output,
                })
            }
        }
    }
}

/// Thin `rskit-worker` pool wrapper.
pub(super) struct ApplyPool {
    pool: Pool<WorkItem, WorkOutcome>,
}

impl ApplyPool {
    /// Create a pool with bounded concurrency.
    #[must_use]
    pub(super) fn new(
        runner: Arc<dyn CommandRunner>,
        max_parallel: usize,
        environment: InvocationEnvironment,
        output: OutputObserver,
        stream_normal_live: bool,
        unit_timeout: Option<Duration>,
    ) -> Self {
        let handler = Arc::new(WorkHandler {
            runner,
            environment,
            output,
            stream_normal_live,
            unit_timeout,
        });
        let config = PoolConfig::new("toven-apply").with_size(max_parallel.max(1));
        Self {
            pool: Pool::new(handler, config),
        }
    }

    /// Submit one work item.
    pub(super) async fn submit(&self, item: WorkItem) -> AppResult<TaskHandle<WorkOutcome>> {
        self.pool.submit(item).await
    }

    /// Stop accepting queued work.
    pub(super) fn close(&self) {
        self.pool.close();
    }

    /// Shut down the underlying worker pool.
    pub(super) async fn shutdown(self) -> AppResult<()> {
        self.pool.shutdown().await
    }
}
