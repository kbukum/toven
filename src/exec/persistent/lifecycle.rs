use std::path::Path;

use crate::{
    core::{AppError, AppResult, ErrorCode, ExecutionUnit},
    exec::{PersistentOutput, RunOptions, RunOutput, process_config, render_execution_unit},
};

use crate::exec::render::argv_field;

use super::{
    command::command_from_argv,
    error::{persistent_exit_result_error, remap_start_error},
    output::map_output,
    readiness::readiness,
};

pub(in crate::exec) struct PersistentProcess {
    unit_id: String,
    process: Option<rskit_process::PersistentProcess>,
    stopped: bool,
}

impl PersistentProcess {
    pub(in crate::exec) fn wait(mut self) -> AppResult<()> {
        self.wait_inner()
    }

    pub(in crate::exec) fn shutdown(mut self) -> AppResult<()> {
        self.shutdown_inner()
    }

    fn wait_inner(&mut self) -> AppResult<()> {
        if self.stopped {
            return Ok(());
        }
        self.stopped = true;
        let result = take_process(&mut self.process)?.wait()?;
        if result.cancelled {
            Err(AppError::new(
                ErrorCode::Cancelled,
                format!("persistent unit '{}' cancelled", self.unit_id),
            ))
        } else if result.success() {
            Ok(())
        } else {
            Err(persistent_exit_result_error(&self.unit_id, &result))
        }
    }

    fn shutdown_inner(&mut self) -> AppResult<()> {
        if self.stopped {
            return Ok(());
        }
        self.stopped = true;
        match take_process(&mut self.process)?.shutdown()? {
            rskit_process::ShutdownOutcome::AlreadyExited(result)
            | rskit_process::ShutdownOutcome::Stopped(result)
                if result.cancelled =>
            {
                Err(AppError::new(
                    ErrorCode::Cancelled,
                    format!("persistent unit '{}' cancelled", self.unit_id),
                ))
            }
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

pub(in crate::exec) struct PersistentRun {
    pub(in crate::exec) output: RunOutput,
    pub(in crate::exec) process: PersistentProcess,
}

#[cfg(test)]
pub(in crate::exec) fn start_persistent_execution_unit(
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

pub(in crate::exec) fn start_persistent_execution_unit_with_output(
    unit: &ExecutionUnit,
    workspace_root: &Path,
    options: &RunOptions,
    output: PersistentOutput,
) -> AppResult<PersistentRun> {
    let argv = render_execution_unit(unit, workspace_root)?;
    let command = command_from_argv(&argv, workspace_root).map_err(|()| {
        AppError::invalid_input(
            argv_field(unit),
            format!("execution unit '{}' rendered an empty argv", unit.id),
        )
    })?;
    let readiness = readiness(unit, workspace_root)?;
    let cancel_token = options
        .cancellation
        .as_ref()
        .map_or_else(tokio_util::sync::CancellationToken::new, |cancellation| {
            cancellation.token()
        });
    let process_config = process_config::captured_config(
        options.timeout,
        rskit_process::InputPolicy::Closed,
        rskit_process::OutputPolicy::captured(),
    );
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
