//! rskit-worker wrapper for already-computed command invocations.

use std::sync::Arc;

use async_trait::async_trait;
use rskit_errors::AppResult;
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
        /// Raw output chunks.
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
}

impl WorkItem {
    /// Build a work item for `unit`.
    #[must_use]
    pub(super) const fn new(unit: ExecutionUnit) -> Self {
        Self { unit }
    }
}

struct WorkHandler {
    runner: Arc<dyn CommandRunner>,
    environment: InvocationEnvironment,
    output: OutputObserver,
    /// Stream normal-unit output live (through `output`) instead of capturing it
    /// for a buffered block. Only set when no two units can run concurrently, so
    /// streamed chunks never interleave.
    stream_normal_live: bool,
}

#[async_trait]
impl Handler<WorkItem, WorkOutcome> for WorkHandler {
    async fn handle(
        &self,
        task: WorkItem,
        _emit: mpsc::Sender<rskit_worker::Event<WorkOutcome>>,
        cancel: CancellationToken,
    ) -> AppResult<WorkOutcome> {
        let invocation = Invocation::from_unit(&task.unit, self.environment.clone());
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
            let outcome = self.runner.run(&invocation, cancel, live).await?;
            Ok(WorkOutcome::Normal {
                success: outcome.success,
                output: outcome.output,
            })
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
    ) -> Self {
        let handler = Arc::new(WorkHandler {
            runner,
            environment,
            output,
            stream_normal_live,
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
