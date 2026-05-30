//! Persistent execution-unit integration.

mod cancel;
mod command;
mod error;
mod output;
mod readiness;
#[cfg(test)]
mod tests;

use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use tokio_util::sync::CancellationToken;

use crate::{
    core::{AppError, AppResult, ErrorCode, ExecutionUnit},
    exec::{PersistentOutput, RunOptions, RunOutput, render_execution_unit},
};

use cancel::{CtrlCHandler, spawn_ctrl_c_handler, stop_ctrl_c_handler};
use command::command_from_argv;
use error::{persistent_exit_result_error, remap_start_error};
use output::map_output;
use readiness::readiness;

pub(super) struct PersistentProcess {
    unit_id: String,
    process: Option<rskit_process::PersistentProcess>,
    ctrl_c_handler: Option<CtrlCHandler>,
    cancelled: Arc<AtomicBool>,
    stopped: bool,
}

impl PersistentProcess {
    pub(super) fn wait(mut self) -> AppResult<()> {
        self.wait_inner()
    }

    pub(super) fn shutdown(mut self) -> AppResult<()> {
        self.shutdown_inner()
    }

    fn wait_inner(&mut self) -> AppResult<()> {
        if self.stopped {
            return Ok(());
        }
        self.stopped = true;
        stop_ctrl_c_handler(self.ctrl_c_handler.take())?;
        let result = take_process(&mut self.process)?.wait()?;
        if result.success() {
            Ok(())
        } else if result.cancelled || self.cancelled.load(Ordering::SeqCst) {
            Err(AppError::new(
                ErrorCode::Cancelled,
                format!("persistent unit '{}' cancelled", self.unit_id),
            ))
        } else {
            Err(persistent_exit_result_error(&self.unit_id, &result))
        }
    }

    fn shutdown_inner(&mut self) -> AppResult<()> {
        if self.stopped {
            return Ok(());
        }
        self.stopped = true;
        stop_ctrl_c_handler(self.ctrl_c_handler.take())?;
        match take_process(&mut self.process)?.shutdown()? {
            rskit_process::ShutdownOutcome::AlreadyExited(result) if !result.success() => {
                Err(persistent_exit_result_error(&self.unit_id, &result))
            }
            _ => Ok(()),
        }
    }
}

impl Drop for PersistentProcess {
    fn drop(&mut self) {
        let _ = self.shutdown_inner();
    }
}

pub(super) struct PersistentRun {
    pub(super) output: RunOutput,
    pub(super) process: PersistentProcess,
}

#[cfg(test)]
fn start_persistent_execution_unit(
    unit: &ExecutionUnit,
    workspace_root: &Path,
    options: &RunOptions,
) -> AppResult<PersistentRun> {
    start_persistent_execution_unit_with_output(
        unit,
        workspace_root,
        options,
        PersistentOutput::capture_only(),
    )
}

pub(super) fn start_persistent_execution_unit_with_output(
    unit: &ExecutionUnit,
    workspace_root: &Path,
    options: &RunOptions,
    output: PersistentOutput,
) -> AppResult<PersistentRun> {
    let argv = render_execution_unit(unit, workspace_root)?;
    let command = command_from_argv(&argv, workspace_root).map_err(|()| {
        AppError::invalid_input(
            "argv",
            format!("execution unit '{}' rendered an empty argv", unit.id),
        )
    })?;
    let readiness = readiness(unit, workspace_root)?;
    let cancel_token = CancellationToken::new();
    let cancelled = Arc::new(AtomicBool::new(false));
    let ctrl_c_handler = if options.cancel_on_ctrl_c {
        Some(spawn_ctrl_c_handler(
            cancel_token.clone(),
            Arc::clone(&cancelled),
        )?)
    } else {
        None
    };
    let process_config = rskit_process::ProcessConfig {
        timeout: options.timeout,
        ..rskit_process::ProcessConfig::default()
    };
    let persistent_config = rskit_process::PersistentConfig::default()
        .with_readiness(readiness)
        .with_readiness_timeout(unit.readiness_timeout)
        .with_output(map_output(output));

    let run = rskit_process::start_persistent_with_cancel(
        &command,
        &process_config,
        &persistent_config,
        cancel_token,
    )
    .map_err(|error| remap_start_error(unit, error))?;

    Ok(PersistentRun {
        output: RunOutput {
            argv,
            result: rskit_process::ProcessResult::completed(
                Some(0),
                run.startup.stdout_bytes,
                run.startup.stderr_bytes,
                run.startup.stdout_truncated,
                run.startup.stderr_truncated,
                run.startup.duration,
                false,
                false,
            ),
            cancelled: false,
        },
        process: PersistentProcess {
            unit_id: unit.id.clone(),
            process: Some(run.process),
            ctrl_c_handler,
            cancelled,
            stopped: false,
        },
    })
}

fn take_process(
    process: &mut Option<rskit_process::PersistentProcess>,
) -> AppResult<rskit_process::PersistentProcess> {
    process
        .take()
        .ok_or_else(|| AppError::new(ErrorCode::Conflict, "persistent process already consumed"))
}
