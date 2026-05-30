//! Subprocess execution for rendered units.

use std::{ffi::OsString, path::Path, time::Duration};

use crate::{
    core::{AppError, AppResult, ErrorCode, ExecutionUnit},
    exec::render_execution_unit,
};

/// Execution options for one unit.
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Optional process timeout.
    pub timeout: Option<Duration>,
    /// Cancel the running process when the CLI receives ctrl-c.
    pub cancel_on_ctrl_c: bool,
}

/// Completed execution output.
#[derive(Debug, Clone)]
pub struct RunOutput {
    /// Rendered argv.
    pub argv: Vec<String>,
    /// Process result.
    pub result: rskit_process::ProcessResult,
    /// Whether the process was cancelled by ctrl-c.
    pub cancelled: bool,
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
    let cancel = tokio_util::sync::CancellationToken::new();
    let (result, cancelled) = if options.cancel_on_ctrl_c {
        runtime.block_on(run_with_ctrl_c_cancel(&command, &process_config, cancel))?
    } else {
        (
            runtime.block_on(rskit_process::run_with_cancel(
                &command,
                &process_config,
                cancel,
            ))?,
            false,
        )
    };
    Ok(RunOutput {
        argv,
        result,
        cancelled,
    })
}

async fn run_with_ctrl_c_cancel(
    command: &rskit_process::Command,
    process_config: &rskit_process::ProcessConfig,
    cancel: tokio_util::sync::CancellationToken,
) -> AppResult<(rskit_process::ProcessResult, bool)> {
    let process = rskit_process::run_with_cancel(command, process_config, cancel.clone());
    tokio::pin!(process);

    tokio::select! {
        result = &mut process => result.map(|result| (result, false)),
        signal = tokio::signal::ctrl_c() => {
            signal.map_err(|error| {
                AppError::new(ErrorCode::Internal, "failed to listen for ctrl-c").with_cause(error)
            })?;
            cancel.cancel();
            process.await.map(|result| (result, true))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RunOptions, run_execution_unit};
    use crate::core::{CommandOrigin, ExecutionMode, ExecutionUnit};

    #[test]
    fn runs_rendered_execution_unit() {
        let root = rskit_testutil::test_workspace!("exec-runner");
        let unit = unit(vec![
            "sh".to_string(),
            "-c".to_string(),
            "printf runner-ok".to_string(),
        ]);

        let output = run_execution_unit(
            &unit,
            root.path(),
            &RunOptions {
                timeout: None,
                cancel_on_ctrl_c: false,
            },
        )
        .expect("execution succeeds");

        assert_eq!(output.argv, ["sh", "-c", "printf runner-ok"]);
        assert_eq!(output.result.stdout, "runner-ok");
        assert!(output.result.success());
        assert!(!output.cancelled);
    }

    #[test]
    fn rejects_empty_rendered_argv() {
        let root = rskit_testutil::test_workspace!("exec-runner-empty");
        let error = run_execution_unit(
            &unit(Vec::new()),
            root.path(),
            &RunOptions {
                timeout: None,
                cancel_on_ctrl_c: false,
            },
        )
        .expect_err("empty argv is rejected");

        assert_eq!(error.code, crate::core::ErrorCode::InvalidInput);
        assert!(error.message.contains("empty argv"));
    }

    fn unit(argv_template: Vec<String>) -> ExecutionUnit {
        ExecutionUnit {
            id: "unit".to_string(),
            profile: "profile".to_string(),
            task: "test".to_string(),
            command_origin: CommandOrigin::DirectArgv,
            mode: ExecutionMode::SpawnEach,
            resource_group: String::new(),
            modules: Vec::new(),
            argv_template,
            module_arg_template: Vec::new(),
            passthrough_args: Vec::new(),
            shared_inputs: Vec::new(),
        }
    }
}
