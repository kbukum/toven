//! Subprocess execution for rendered units.

use std::{ffi::OsString, path::Path, time::Duration};

use tokio_util::sync::CancellationToken;

use crate::{
    core::{AppError, AppResult, ErrorCode, ExecutionUnit},
    exec::render_execution_unit,
};

/// Execution options for one unit.
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Optional process timeout.
    pub timeout: Option<Duration>,
    /// Cancellation token shared with CLI signal handling.
    pub cancel: CancellationToken,
}

/// Completed execution output.
#[derive(Debug, Clone)]
pub struct RunOutput {
    /// Rendered argv.
    pub argv: Vec<String>,
    /// Process result.
    pub result: rskit_process::ProcessResult,
}

/// Render and execute one execution unit.
pub fn run_execution_unit(
    unit: &ExecutionUnit,
    workspace_root: &Path,
    options: &RunOptions,
) -> AppResult<RunOutput> {
    let argv = render_execution_unit(unit, workspace_root)?;
    let Some((program, arguments)) = argv.split_first() else {
        return Err(AppError::invalid_input(
            "argv",
            format!("execution unit '{}' rendered an empty argv", unit.id),
        ));
    };
    let command = rskit_process::Command::new(program)
        .args(arguments.iter().map(OsString::from))
        .dir(workspace_root.to_path_buf());
    let process_config = rskit_process::ProcessConfig {
        timeout: options.timeout,
        ..rskit_process::ProcessConfig::default()
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            AppError::new(ErrorCode::Internal, "failed to create process runtime").with_cause(error)
        })?;
    let result = runtime.block_on(rskit_process::run_with_cancel(
        &command,
        &process_config,
        options.cancel.clone(),
    ))?;
    Ok(RunOutput { argv, result })
}
