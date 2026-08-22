//! rskit-worker wrapper for already-computed command invocations.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rskit_errors::{AppResult, ErrorCode};
use rskit_worker::{Handler, Pool, PoolConfig, TaskHandle};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use toven_model::{ExecutionUnit, UnitOutput};
use toven_ports::{
    CommandRunner, Invocation, InvocationEnvPolicy, InvocationEnvironment, OutputObserver,
    StartOutcome,
};

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
    /// same name — the base takes precedence). A base var can be set explicitly
    /// in `self.environment.vars` or, under `InheritParent`, come from the
    /// parent process environment; both win over the injected share.
    fn environment_for(&self, task: &WorkItem) -> InvocationEnvironment {
        merge_extra_env(&self.environment, &task.extra_env, |name| {
            std::env::var_os(name).is_some_and(|value| !value.is_empty())
        })
    }
}

/// Merge a unit's compute-budget `extra_env` on top of `base`, never clobbering
/// an operator-set base var of the same name.
///
/// A base var can be set explicitly in `base.vars` or, under `InheritParent`,
/// come from the parent process environment; both win over the injected share.
/// `parent_has_nonempty` reports whether a name is present and non-empty in the
/// inherited parent environment — injected rather than read from
/// [`std::env`] here so the precedence stays pure and hermetically testable
/// without touching process-global state.
fn merge_extra_env(
    base: &InvocationEnvironment,
    extra_env: &BTreeMap<String, String>,
    parent_has_nonempty: impl Fn(&str) -> bool,
) -> InvocationEnvironment {
    if extra_env.is_empty() {
        return base.clone();
    }
    let inherits_parent = base.policy == InvocationEnvPolicy::InheritParent;
    let mut environment = base.clone();
    for (name, value) in extra_env {
        // Skip an inherited parent var of the same name: it is not present in
        // `vars`, so `entry(..).or_insert` alone would still shadow it.
        if inherits_parent && parent_has_nonempty(name) {
            continue;
        }
        environment
            .vars
            .entry(name.clone())
            .or_insert_with(|| value.clone());
    }
    environment
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use toven_ports::InvocationEnvironment;

    use super::merge_extra_env;

    fn extra() -> BTreeMap<String, String> {
        BTreeMap::from([("GOMAXPROCS".to_string(), "3".to_string())])
    }

    #[test]
    fn injects_the_share_when_no_base_var_exists() {
        // InheritParent with the name absent from both `vars` and the parent
        // env → the computed share is injected.
        let base = InvocationEnvironment::inherit_parent(BTreeMap::new());
        let merged = merge_extra_env(&base, &extra(), |_| false);
        assert_eq!(merged.vars.get("GOMAXPROCS").map(String::as_str), Some("3"));
    }

    #[test]
    fn an_explicit_base_var_wins_over_the_injected_share() {
        // The name is set explicitly in `vars`; `or_insert` must not overwrite
        // it regardless of the parent probe.
        let base = InvocationEnvironment::inherit_parent(BTreeMap::from([(
            "GOMAXPROCS".to_string(),
            "7".to_string(),
        )]));
        let merged = merge_extra_env(&base, &extra(), |_| false);
        assert_eq!(merged.vars.get("GOMAXPROCS").map(String::as_str), Some("7"));
    }

    #[test]
    fn a_non_empty_inherited_parent_var_wins_over_the_injected_share() {
        // InheritParent and the parent holds a non-empty value (modeled by the
        // injected probe): the base wins, so nothing is added to `vars`.
        let base = InvocationEnvironment::inherit_parent(BTreeMap::new());
        let merged = merge_extra_env(&base, &extra(), |name| name == "GOMAXPROCS");
        assert!(
            !merged.vars.contains_key("GOMAXPROCS"),
            "an inherited parent var must not be shadowed by the injected share",
        );
    }

    #[test]
    fn a_non_inherit_policy_ignores_the_parent_and_injects() {
        // Without InheritParent the parent env is irrelevant, so the share is
        // injected even when the probe would report the name present.
        let base = InvocationEnvironment::explicit(BTreeMap::new());
        let merged = merge_extra_env(&base, &extra(), |_| true);
        assert_eq!(merged.vars.get("GOMAXPROCS").map(String::as_str), Some("3"));
    }
}
