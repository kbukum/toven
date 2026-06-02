//! Subprocess execution for rendered units.

use std::{
    ffi::OsString,
    io::{ErrorKind, Write as _},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use crate::{
    core::process_config::{captured_config, observed_config},
    core::{AppError, AppResult, ErrorCode, ExecutionUnit},
    exec::SharedCancellation,
};

use super::render::{argv_field, render_execution_unit};

/// Execution options for one unit.
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Optional process timeout.
    pub timeout: Option<Duration>,
    /// Shared cancellation token for externally coordinated shutdown.
    pub(crate) cancellation: Option<SharedCancellation>,
    /// Stream child process stdout/stderr lines to parent streams in real time.
    pub stream_output: bool,
}

/// Completed execution output.
#[derive(Debug, Clone)]
pub struct RunOutput {
    /// Rendered argv.
    pub argv: Vec<String>,
    /// Process result.
    pub result: rskit_process::ProcessResult,
    /// Whether the process was cancelled by the caller.
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
            argv_field(unit),
            format!("execution unit '{}' rendered an empty argv", unit.id),
        ));
    };
    let command = rskit_process::ProcessSpec::new(program)
        .args(arguments.iter().map(OsString::from))
        .dir(workspace_root.to_path_buf());
    let process_config = captured_config(
        options.timeout,
        rskit_process::InputPolicy::Closed,
        rskit_process::OutputPolicy::captured(),
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            AppError::new(ErrorCode::Internal, "failed to create process runtime").with_cause(error)
        })?;
    let cancel = options.cancellation.as_ref().map_or_else(
        tokio_util::sync::CancellationToken::new,
        SharedCancellation::token,
    );
    let result = runtime.block_on(run_with_optional_streaming(
        &command,
        &process_config,
        cancel,
        options.stream_output,
    ))?;
    let cancelled = result.cancelled;
    Ok(RunOutput {
        argv,
        result,
        cancelled,
    })
}

async fn run_with_optional_streaming(
    command: &rskit_process::ProcessSpec,
    process_config: &rskit_process::ProcessConfig,
    cancel: tokio_util::sync::CancellationToken,
    stream_output: bool,
) -> AppResult<rskit_process::ProcessResult> {
    if !stream_output {
        return rskit_process::run_with_cancel(command, process_config, cancel).await;
    }

    let stdout_cancel = cancel.clone();
    let stderr_cancel = cancel.clone();
    let stdout_open = Arc::new(AtomicBool::new(true));
    let stderr_open = Arc::new(AtomicBool::new(true));
    let observer = rskit_process::OutputObserver::new()
        .with_stdout_bytes({
            let stdout_open = Arc::clone(&stdout_open);
            move |bytes| {
                if !stdout_open.load(Ordering::Relaxed) {
                    return;
                }
                let mut stdout = std::io::stdout().lock();
                if let Err(error) = stdout.write_all(bytes).and_then(|()| stdout.flush()) {
                    if error.kind() == ErrorKind::BrokenPipe {
                        stdout_open.store(false, Ordering::Relaxed);
                    } else {
                        stdout_cancel.cancel();
                    }
                }
            }
        })
        .with_stderr_bytes({
            let stderr_open = Arc::clone(&stderr_open);
            move |bytes| {
                if !stderr_open.load(Ordering::Relaxed) {
                    return;
                }
                let mut stderr = std::io::stderr().lock();
                if let Err(error) = stderr.write_all(bytes).and_then(|()| stderr.flush()) {
                    if error.kind() == ErrorKind::BrokenPipe {
                        stderr_open.store(false, Ordering::Relaxed);
                    } else {
                        stderr_cancel.cancel();
                    }
                }
            }
        });
    let config = observed_config(
        process_config.timeout,
        rskit_process::InputPolicy::Closed,
        rskit_process::OutputPolicy::observe_only(),
        observer,
    );

    rskit_process::run_with_cancel(command, &config, cancel).await
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
                cancellation: None,
                stream_output: false,
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
                cancellation: None,
                stream_output: false,
            },
        )
        .expect_err("empty argv is rejected");

        assert_eq!(error.code, crate::core::ErrorCode::InvalidInput);
        assert!(error.message.contains("scopes.profile.tasks.test.argv"));
        assert!(error.message.contains("empty argv"));
    }

    fn unit(argv_template: Vec<String>) -> ExecutionUnit {
        ExecutionUnit {
            id: "unit".to_string(),
            scope_id: crate::core::ScopeId::new("profile").expect("scope id"),
            adapter_id: crate::core::AdapterId::new("rust").expect("adapter id"),
            task: "test".to_string(),
            command_origin: CommandOrigin::DirectArgv,
            task_origin: crate::core::TaskOrigin::ProjectDefault,
            mode: ExecutionMode::SpawnEach,
            resource_group: String::new(),
            modules: Vec::new(),
            argv_template,
            module_arg_template: Vec::new(),
            passthrough_args: Vec::new(),
            toolchain_probes: Vec::new(),
            cache_args: false,
            persistent: false,
            readiness: crate::core::PersistentReadiness::Started,
            readiness_timeout: std::time::Duration::from_secs(30),
            shared_inputs: Vec::new(),
        }
    }
}
