//! Persistent process readiness and held-process lifecycle over rskit-process.

use std::path::Path;
use std::sync::{Arc, Mutex};

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_process::{
    PersistentConfig, PersistentOutputObserver, PersistentReadiness, ProcessConfig,
    persistent_start_error_kind, start_persistent_with_cancel,
};
use tokio_util::sync::CancellationToken;
use toven_model::{ExecutionReadiness, OutputStream, UnitOutput};
use toven_ports::{HeldProcess, Invocation, OutputObserver, StartOutcome};

use crate::apply::exec::spec;

/// Start a persistent invocation and wait until readiness succeeds or fails.
pub(in crate::apply) async fn start_persistent(
    invocation: &Invocation,
    project_root: &Path,
    process_config: &ProcessConfig,
    shutdown_grace: std::time::Duration,
    cancel: CancellationToken,
    output: OutputObserver,
) -> AppResult<StartOutcome> {
    let spec = spec(invocation, project_root)?;
    let persistent_config = PersistentConfig::default()
        .with_readiness(readiness(invocation, project_root)?)
        .with_readiness_timeout(invocation.readiness_timeout)
        .with_shutdown_grace_period(shutdown_grace)
        .with_output_observer(process_observer(&invocation.unit_id, output));
    let unit_id = invocation.unit_id.clone();
    let process_config = process_config.clone();
    let run = tokio::task::spawn_blocking(move || {
        start_persistent_with_cancel(&spec, &process_config, &persistent_config, cancel)
    })
    .await
    .map_err(AppError::internal)?;

    match run {
        Ok(run) => Ok(StartOutcome::Ready {
            output: Vec::new(),
            process: Box::new(ProcessHeldProcess {
                unit_id,
                process: Arc::new(Mutex::new(Some(run.process))),
            }),
        }),
        Err(error)
            if persistent_start_error_kind(&error).is_some()
                || matches!(error.code(), ErrorCode::Timeout) =>
        {
            Ok(StartOutcome::FailedReadiness {
                output: readiness_error_output(&unit_id, &error),
            })
        }
        Err(error) => Err(error),
    }
}

struct ProcessHeldProcess {
    unit_id: String,
    process: Arc<Mutex<Option<rskit_process::PersistentProcess>>>,
}

impl HeldProcess for ProcessHeldProcess {
    fn unit_id(&self) -> &str {
        &self.unit_id
    }

    fn shutdown(self: Box<Self>) -> AppResult<()> {
        let process = self
            .process
            .lock()
            .map_err(|_| AppError::new(ErrorCode::Internal, "persistent process lock poisoned"))?
            .take();
        if let Some(process) = process {
            process.shutdown()?;
        }
        Ok(())
    }
}

fn readiness(
    invocation: &Invocation,
    project_root: &Path,
) -> AppResult<PersistentReadiness> {
    match &invocation.readiness {
        ExecutionReadiness::Started => Ok(PersistentReadiness::Started),
        ExecutionReadiness::OutputContains(value) => {
            Ok(PersistentReadiness::OutputContains(value.clone()))
        }
        ExecutionReadiness::Command(argv) => {
            // Run the readiness probe under the same explicit environment as the
            // main invocation so it inherits the task's PATH allowlist and vars;
            // otherwise common probe tools (`curl`, `sh`, …) may fail to spawn
            // even when the persistent command itself runs fine.
            let probe = Invocation::new("readiness", argv.clone())
                .with_environment(invocation.environment.clone());
            spec(&probe, project_root).map(PersistentReadiness::Command)
        }
    }
}

fn process_observer(unit_id: &str, output: OutputObserver) -> PersistentOutputObserver {
    let stdout_unit = unit_id.to_string();
    let stderr_unit = unit_id.to_string();
    PersistentOutputObserver::new()
        .with_stdout_bytes({
            let output = output.clone();
            move |bytes| {
                output.emit(UnitOutput {
                    unit_id: stdout_unit.clone(),
                    stream: OutputStream::Stdout,
                    bytes: bytes.to_vec(),
                });
            }
        })
        .with_stderr_bytes(move |bytes| {
            output.emit(UnitOutput {
                unit_id: stderr_unit.clone(),
                stream: OutputStream::Stderr,
                bytes: bytes.to_vec(),
            });
        })
}

fn readiness_error_output(unit_id: &str, error: &AppError) -> Vec<UnitOutput> {
    vec![UnitOutput {
        unit_id: unit_id.to_string(),
        stream: OutputStream::Stderr,
        bytes: error.to_string().into_bytes(),
    }]
}
