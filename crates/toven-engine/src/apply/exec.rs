//! Concrete rskit-process-backed [`CommandRunner`](toven_ports::CommandRunner).

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use rskit_errors::{AppError, AppResult};
use rskit_process::{
    CapturedIo, EnvPolicy, ObservedIo, OutputObserver as ProcessOutputObserver, OutputPolicy,
    ProcessConfig, ProcessIo, ProcessSpec, SignalPolicy, run_with_cancel,
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

    /// Capture stdout/stderr, returning the full output once the process exits.
    /// Used when output must be buffered into a deterministic per-unit block
    /// (the default under parallelism).
    async fn run_captured(
        &self,
        invocation: &Invocation,
        spec: ProcessSpec,
        cancel: CancellationToken,
    ) -> AppResult<RunOutcome> {
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

    /// Stream stdout/stderr through `observer` as the process runs (no capture),
    /// returning an empty [`RunOutcome::output`] because the bytes were already
    /// surfaced live. Used when no two units can run concurrently.
    async fn run_streaming(
        &self,
        invocation: &Invocation,
        spec: ProcessSpec,
        cancel: CancellationToken,
        observer: OutputObserver,
    ) -> AppResult<RunOutcome> {
        let io = ObservedIo::new(streaming_observer(invocation.unit_id.clone(), observer))
            .with_output(OutputPolicy::observe_only());
        let config = self.process_config.clone().with_io(ProcessIo::observed(io));
        let result = run_with_cancel(&spec, &config, cancel).await?;
        if result.success() {
            Ok(RunOutcome::succeeded(Vec::new()))
        } else {
            Ok(RunOutcome::failed(result.exit_code, Vec::new()))
        }
    }
}

#[async_trait]
impl CommandRunner for ProcessCommandRunner {
    async fn run(
        &self,
        invocation: &Invocation,
        cancel: CancellationToken,
        live: Option<OutputObserver>,
    ) -> AppResult<RunOutcome> {
        let spec = spec(invocation, &self.project_root)?;
        match live {
            Some(observer) => self.run_streaming(invocation, spec, cancel, observer).await,
            None => self.run_captured(invocation, spec, cancel).await,
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

/// Build an `rskit-process` observer that forwards each raw stdout/stderr chunk
/// to `observer` as a [`UnitOutput`] tagged with `unit_id`, so the live bridge
/// streams it while the process is still running.
fn streaming_observer(unit_id: String, observer: OutputObserver) -> ProcessOutputObserver {
    let stdout_id = unit_id.clone();
    let stdout_sink = observer.clone();
    let stderr_id = unit_id;
    let stderr_sink = observer;
    ProcessOutputObserver::new()
        .with_stdout_bytes(move |bytes| {
            stdout_sink.emit(UnitOutput {
                unit_id: stdout_id.clone(),
                stream: OutputStream::Stdout,
                bytes: bytes.to_vec(),
            });
        })
        .with_stderr_bytes(move |bytes| {
            stderr_sink.emit(UnitOutput {
                unit_id: stderr_id.clone(),
                stream: OutputStream::Stderr,
                bytes: bytes.to_vec(),
            });
        })
}
