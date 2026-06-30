//! Concrete rskit-process-backed [`CommandRunner`](toven_ports::CommandRunner).

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use rskit_errors::{AppError, AppResult};
use rskit_process::{
    CapturedIo, EnvPolicy, ProcessConfig, ProcessIo, ProcessSpec, SignalPolicy, run_with_cancel,
};
use tokio_util::sync::CancellationToken;
use toven_model::{OutputStream, UnitOutput};
use toven_ports::{
    CommandRunner, Invocation, InvocationEnvPolicy, OutputObserver, RunOutcome, StartOutcome,
};

use super::persistent::lifecycle;

/// Runs command invocations with `rskit-process`.
pub struct ProcessCommandRunner {
    project_root: PathBuf,
    process_config: ProcessConfig,
    persistent_shutdown_grace: std::time::Duration,
}

impl ProcessCommandRunner {
    /// Create a process runner rooted at `project_root`.
    #[must_use]
    pub fn new(project_root: impl Into<PathBuf>) -> Self {
        let process_config = ProcessConfig::default()
            .with_io(ProcessIo::captured(CapturedIo::new()))
            .with_signal_policy(SignalPolicy::default());
        Self {
            project_root: project_root.into(),
            process_config,
            persistent_shutdown_grace: std::time::Duration::from_secs(5),
        }
    }

    /// Override the process policy used for normal and persistent commands.
    #[must_use]
    pub fn with_process_config(mut self, config: ProcessConfig) -> Self {
        self.process_config = config;
        self
    }

    /// Override the persistent shutdown grace period.
    #[must_use]
    pub const fn with_persistent_shutdown_grace(mut self, grace: std::time::Duration) -> Self {
        self.persistent_shutdown_grace = grace;
        self
    }
}

#[async_trait]
impl CommandRunner for ProcessCommandRunner {
    async fn run(
        &self,
        invocation: &Invocation,
        cancel: CancellationToken,
    ) -> AppResult<RunOutcome> {
        let spec = spec(invocation, &self.project_root)?;
        let result = run_with_cancel(&spec, &self.process_config, cancel).await?;
        if result.stdout_truncated || result.stderr_truncated {
            return Err(AppError::new(
                rskit_errors::ErrorCode::Internal,
                format!(
                    "unit '{}' exceeded its captured-output bound (stdout_truncated={}, stderr_truncated={})",
                    invocation.unit_id, result.stdout_truncated, result.stderr_truncated
                ),
            ));
        }
        let output = output(
            invocation.unit_id.as_str(),
            &result.stdout_bytes,
            &result.stderr_bytes,
        );
        if result.success() {
            Ok(RunOutcome::succeeded(output))
        } else {
            Ok(RunOutcome::failed(result.exit_code, output))
        }
    }

    async fn start_persistent(
        &self,
        invocation: &Invocation,
        cancel: CancellationToken,
        output: OutputObserver,
    ) -> AppResult<StartOutcome> {
        lifecycle::start_persistent(
            invocation,
            &self.project_root,
            &self.process_config,
            self.persistent_shutdown_grace,
            cancel,
            output,
        )
        .await
    }
}

pub(super) fn spec(invocation: &Invocation, project_root: &Path) -> AppResult<ProcessSpec> {
    let (program, args) = invocation
        .argv
        .split_first()
        .ok_or_else(|| AppError::invalid_input("argv", "must include a program"))?;
    Ok(ProcessSpec::new(program)
        .args(args.iter().cloned())
        .dir(project_root)
        .env_policy(match invocation.environment.policy {
            InvocationEnvPolicy::ExplicitOnly => EnvPolicy::Empty,
            InvocationEnvPolicy::InheritParent => EnvPolicy::Inherit,
        })
        .envs(invocation.environment.vars.clone()))
}

fn output(unit_id: &str, stdout: &[u8], stderr: &[u8]) -> Vec<UnitOutput> {
    let mut output = Vec::new();
    if !stdout.is_empty() {
        output.push(UnitOutput {
            unit_id: unit_id.to_string(),
            stream: OutputStream::Stdout,
            bytes: stdout.to_vec(),
        });
    }
    if !stderr.is_empty() {
        output.push(UnitOutput {
            unit_id: unit_id.to_string(),
            stream: OutputStream::Stderr,
            bytes: stderr.to_vec(),
        });
    }
    output
}
