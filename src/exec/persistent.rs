//! Persistent execution-unit integration.

use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use tokio_util::sync::CancellationToken;

use crate::{
    core::{AppError, AppResult, ErrorCode, ExecutionUnit, PersistentReadiness},
    exec::{
        PersistentOutput, PersistentOutputStream, RunOptions, RunOutput, render_execution_unit,
    },
};

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
            rskit_process::ShutdownOutcome::AlreadyExited(result) => {
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

fn command_from_argv(argv: &[String], workspace_root: &Path) -> Result<rskit_process::Command, ()> {
    let Some((program, arguments)) = argv.split_first() else {
        return Err(());
    };
    Ok(rskit_process::Command::new(program)
        .args(arguments.iter().map(std::ffi::OsString::from))
        .dir(workspace_root))
}

fn readiness(
    unit: &ExecutionUnit,
    workspace_root: &Path,
) -> AppResult<rskit_process::PersistentReadiness> {
    match &unit.readiness {
        PersistentReadiness::Started => Ok(rskit_process::PersistentReadiness::Started),
        PersistentReadiness::OutputContains(value) => Ok(
            rskit_process::PersistentReadiness::OutputContains(value.clone()),
        ),
        PersistentReadiness::Command(argv_template) => {
            let mut ready_unit = unit.clone();
            ready_unit.argv_template.clone_from(argv_template);
            let argv = render_execution_unit(&ready_unit, workspace_root)?;
            let command = command_from_argv(&argv, workspace_root).map_err(|()| {
                AppError::invalid_input(
                    "readiness.argv",
                    format!(
                        "persistent unit '{}' rendered an empty readiness argv",
                        unit.id
                    ),
                )
            })?;
            Ok(rskit_process::PersistentReadiness::Command(command))
        }
    }
}

fn remap_start_error(unit: &ExecutionUnit, error: AppError) -> AppError {
    if error.message.contains("failed to spawn persistent process") {
        return AppError::new(
            error.code,
            format!("failed to spawn persistent unit '{}'", unit.id),
        )
        .with_cause(error);
    }
    if error.message.contains("readiness command timed out") {
        return AppError::new(
            ErrorCode::Timeout,
            format!("persistent unit '{}' readiness command timed out", unit.id),
        )
        .with_cause(error);
    }
    if error.message.contains("readiness command failed") {
        return AppError::new(
            ErrorCode::Internal,
            format!("persistent unit '{}' readiness command failed", unit.id),
        )
        .with_cause(error);
    }
    if error.message.contains("did not become ready") {
        return AppError::new(
            ErrorCode::Timeout,
            format!("persistent unit '{}' did not become ready", unit.id),
        )
        .with_cause(error);
    }
    if error.message.contains("output ended before readiness") {
        return AppError::new(
            ErrorCode::Internal,
            format!(
                "persistent unit '{}' output ended before readiness was observed",
                unit.id
            ),
        )
        .with_cause(error);
    }
    if error.message.contains("exited unexpectedly") {
        return AppError::new(
            ErrorCode::Internal,
            format!("persistent unit '{}' exited unexpectedly", unit.id),
        )
        .with_cause(error);
    }
    error
}

fn persistent_exit_result_error(unit_id: &str, result: &rskit_process::ProcessResult) -> AppError {
    AppError::new(
        ErrorCode::Internal,
        format!(
            "persistent unit '{unit_id}' exited unexpectedly with status {:?}",
            result.exit_code
        ),
    )
}

fn take_process(
    process: &mut Option<rskit_process::PersistentProcess>,
) -> AppResult<rskit_process::PersistentProcess> {
    process
        .take()
        .ok_or_else(|| AppError::new(ErrorCode::Conflict, "persistent process already consumed"))
}

struct CtrlCHandler {
    token: CancellationToken,
    thread: thread::JoinHandle<AppResult<()>>,
}

fn spawn_ctrl_c_handler(
    cancel_token: CancellationToken,
    cancelled: Arc<AtomicBool>,
) -> AppResult<CtrlCHandler> {
    let token = CancellationToken::new();
    let wait_token = token.clone();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            AppError::new(ErrorCode::Internal, "failed to create ctrl-c runtime").with_cause(error)
        })?;
    let thread = thread::spawn(move || {
        runtime.block_on(async move {
            tokio::select! {
                signal = tokio::signal::ctrl_c() => {
                    signal.map_err(|error| {
                        AppError::new(ErrorCode::Internal, "failed to listen for ctrl-c").with_cause(error)
                    })?;
                    cancelled.store(true, Ordering::SeqCst);
                    cancel_token.cancel();
                    Ok(())
                }
                () = wait_token.cancelled() => Ok(()),
            }
        })
    });
    Ok(CtrlCHandler { token, thread })
}

fn stop_ctrl_c_handler(handler: Option<CtrlCHandler>) -> AppResult<()> {
    if let Some(handler) = handler {
        handler.token.cancel();
        handler
            .thread
            .join()
            .map_err(|_| AppError::new(ErrorCode::Internal, "persistent ctrl-c handler panicked"))?
    } else {
        Ok(())
    }
}

const fn map_output(output: PersistentOutput) -> rskit_process::PersistentOutput {
    match (output.stdout_stream(), output.stderr_stream()) {
        (Some(stdout), Some(stderr)) => {
            rskit_process::PersistentOutput::forward(map_stream(stdout), map_stream(stderr))
        }
        _ => rskit_process::PersistentOutput::capture_only(),
    }
}

const fn map_stream(stream: PersistentOutputStream) -> rskit_process::PersistentOutputStream {
    match stream {
        PersistentOutputStream::Stdout => rskit_process::PersistentOutputStream::Stdout,
        PersistentOutputStream::Stderr => rskit_process::PersistentOutputStream::Stderr,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::{
        core::{CommandOrigin, ExecutionMode, ExecutionUnit, PersistentReadiness},
        exec::RunOptions,
    };

    use super::start_persistent_execution_unit;

    #[test]
    fn output_matcher_marks_persistent_unit_ready() {
        let root = rskit_testutil::test_workspace!("persistent-ready-output");
        let mut unit = unit();
        unit.argv_template = vec![
            "sh".to_string(),
            "-c".to_string(),
            "printf listening; sleep 2".to_string(),
        ];
        unit.readiness = PersistentReadiness::OutputContains("listening".to_string());

        let persistent = start_persistent_execution_unit(
            &unit,
            root.path(),
            &RunOptions {
                timeout: None,
                cancel_on_ctrl_c: false,
            },
        )
        .expect("persistent unit becomes ready");

        assert!(persistent.output.result.stdout.contains("listening"));
    }

    #[test]
    fn readiness_timeout_fails_persistent_unit() {
        let root = rskit_testutil::test_workspace!("persistent-ready-timeout");
        let mut unit = unit();
        unit.readiness = PersistentReadiness::OutputContains("never".to_string());
        unit.readiness_timeout = Duration::from_millis(20);

        let result = start_persistent_execution_unit(
            &unit,
            root.path(),
            &RunOptions {
                timeout: None,
                cancel_on_ctrl_c: false,
            },
        );
        let Err(error) = result else {
            panic!("readiness should time out");
        };

        assert_eq!(error.code, crate::core::ErrorCode::Timeout);
    }

    #[test]
    fn early_exit_before_readiness_reports_process_status() {
        let root = rskit_testutil::test_workspace!("persistent-early-exit-before-ready");
        let mut unit = unit();
        unit.argv_template = vec!["sh".to_string(), "-c".to_string(), "exit 7".to_string()];
        unit.readiness = PersistentReadiness::OutputContains("never".to_string());

        let result = start_persistent_execution_unit(
            &unit,
            root.path(),
            &RunOptions {
                timeout: None,
                cancel_on_ctrl_c: false,
            },
        );
        let Err(error) = result else {
            panic!("early exit should fail readiness");
        };

        assert_eq!(error.code, crate::core::ErrorCode::Internal);
        assert!(error.message.contains("exited unexpectedly"));
    }

    #[test]
    fn empty_persistent_argv_reports_invalid_input() {
        let root = rskit_testutil::test_workspace!("persistent-empty-argv");
        let mut unit = unit();
        unit.argv_template = Vec::new();

        let result = start_persistent_execution_unit(
            &unit,
            root.path(),
            &RunOptions {
                timeout: None,
                cancel_on_ctrl_c: false,
            },
        );
        let Err(error) = result else {
            panic!("empty argv should fail");
        };

        assert_eq!(error.code, crate::core::ErrorCode::InvalidInput);
        assert!(error.message.contains("rendered an empty argv"));
    }

    #[test]
    fn spawn_failure_reports_persistent_unit() {
        let root = rskit_testutil::test_workspace!("persistent-spawn-failure");
        let mut unit = unit();
        unit.argv_template = vec!["/definitely/not/a/toven-command".to_string()];

        let result = start_persistent_execution_unit(
            &unit,
            root.path(),
            &RunOptions {
                timeout: None,
                cancel_on_ctrl_c: false,
            },
        );
        let Err(error) = result else {
            panic!("spawn failure should fail");
        };

        assert_eq!(error.code, crate::core::ErrorCode::Internal);
        assert!(error.message.contains("failed to spawn persistent unit"));
    }

    #[test]
    fn readiness_command_success_marks_unit_ready() {
        let root = rskit_testutil::test_workspace!("persistent-ready-command-success");
        let mut unit = unit();
        unit.argv_template = vec!["sleep".to_string(), "2".to_string()];
        unit.readiness = PersistentReadiness::Command(vec!["true".to_string()]);

        let persistent = start_persistent_execution_unit(
            &unit,
            root.path(),
            &RunOptions {
                timeout: None,
                cancel_on_ctrl_c: false,
            },
        )
        .expect("readiness command succeeds");

        persistent.process.shutdown().expect("process shuts down");
    }

    #[test]
    fn readiness_command_failure_reports_failure() {
        let root = rskit_testutil::test_workspace!("persistent-ready-command-failure");
        let mut unit = unit();
        unit.readiness = PersistentReadiness::Command(vec!["false".to_string()]);

        let result = start_persistent_execution_unit(
            &unit,
            root.path(),
            &RunOptions {
                timeout: None,
                cancel_on_ctrl_c: false,
            },
        );
        let Err(error) = result else {
            panic!("readiness command should fail");
        };

        assert_eq!(error.code, crate::core::ErrorCode::Internal);
        assert!(error.message.contains("readiness command failed"));
    }

    #[test]
    fn readiness_command_uses_readiness_timeout() {
        let root = rskit_testutil::test_workspace!("persistent-ready-command-timeout");
        let mut unit = unit();
        unit.readiness = PersistentReadiness::Command(vec!["sleep".to_string(), "2".to_string()]);
        unit.readiness_timeout = Duration::from_millis(20);

        let result = start_persistent_execution_unit(
            &unit,
            root.path(),
            &RunOptions {
                timeout: None,
                cancel_on_ctrl_c: false,
            },
        );

        let Err(error) = result else {
            panic!("readiness command should time out");
        };
        assert_eq!(error.code, crate::core::ErrorCode::Timeout);
        assert!(error.message.contains("readiness command timed out"));
    }

    fn unit() -> ExecutionUnit {
        ExecutionUnit {
            id: "dev/server/workspace".to_string(),
            profile: "dev".to_string(),
            task: "server".to_string(),
            command_origin: CommandOrigin::DirectArgv,
            mode: ExecutionMode::WorkspaceOnce,
            resource_group: String::new(),
            modules: Vec::new(),
            argv_template: vec!["sh".to_string(), "-c".to_string(), "sleep 2".to_string()],
            module_arg_template: Vec::new(),
            passthrough_args: Vec::new(),
            cache_args: false,
            persistent: true,
            readiness: PersistentReadiness::Started,
            readiness_timeout: Duration::from_secs(2),
            shared_inputs: Vec::new(),
        }
    }
}
