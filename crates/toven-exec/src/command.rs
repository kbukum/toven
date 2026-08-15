//! The async streaming [`ProcessCommandRunner`] and the persistent-process
//! spawn helper, both backed by the rskit process port.
//!
//! Peers of the synchronous [`ProcessToolRunner`](super::ProcessToolRunner):
//! they drive the wave-oriented APPLY shape (streaming/cancellable capture,
//! the `fail_if_output` gate, persistent readiness) rather than the one-shot
//! captured shape, but they share the same argv→[`ProcessSpec`] lowering
//! ([`base_spec`](super::spec::base_spec)) so the argv guard and env-policy
//! mapping live in exactly one place. The engine APPLY walk composes these
//! runners; the held-set/teardown orchestration around a returned
//! [`HeldProcess`] stays in the engine.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_process::{
    CapturedIo, ObservedIo, OutputObserver as ProcessOutputObserver, OutputPolicy,
    PersistentConfig, PersistentOutputObserver, PersistentReadiness, ProcessConfig, ProcessIo,
    ProcessSpec, ProcessSupervisor, persistent_start_error_kind, run_with_cancel_supervised,
    start_persistent_supervised,
};
#[cfg(unix)]
use rskit_process::{PtyIo, PtySize};
use tokio_util::sync::CancellationToken;
use toven_model::{ExecutionReadiness, OutputStream, UnitOutput};
use toven_ports::{
    CommandRunner, HeldProcess, Invocation, OutputObserver, RunOutcome, StartOutcome,
};

/// Runs command invocations with `rskit-process`.
pub struct ProcessCommandRunner {
    project_root: PathBuf,
    process_config: ProcessConfig,
    supervisor: Arc<ProcessSupervisor>,
    #[cfg(unix)]
    pty_size: Option<PtySize>,
}

impl ProcessCommandRunner {
    /// Create a process runner rooted at `project_root`.
    #[must_use]
    pub fn new(project_root: impl Into<PathBuf>) -> Self {
        // `ProcessConfig::default()` carries rskit-process's own 30s process timeout,
        // but Toven owns the per-unit bound: it defaults to *unbounded* and is applied
        // cooperatively in the APPLY pool only when the caller passes `--timeout`.
        // Clear the inherited default so a long build/test unit is never silently
        // killed at 30s behind Toven's back.
        let process_config = ProcessConfig::default()
            .with_timeout(None)
            .with_io(ProcessIo::captured(CapturedIo::new()));
        Self {
            project_root: project_root.into(),
            process_config,
            supervisor: Arc::new(ProcessSupervisor::new(
                rskit_process::LifecyclePolicy::default(),
            )),
            #[cfg(unix)]
            pty_size: None,
        }
    }

    /// Enable PTY streaming sized to `terminal` when it is a real terminal.
    ///
    /// Live-streamed units then render exactly as they would interactively
    /// (colors, progress bars, tty-gated styling). When `terminal` is not a tty
    /// (output redirected or captured), this is a no-op and the runner keeps
    /// deterministic pipe capture. On non-Unix targets PTY support is not yet
    /// available, so this is always a no-op regardless of `terminal`.
    #[cfg(unix)]
    #[must_use]
    pub fn with_pty_matching_terminal(self, terminal: &impl std::os::unix::io::AsRawFd) -> Self {
        match rskit_process::terminal_size(terminal) {
            Some(size) => self.with_pty(size),
            None => self,
        }
    }

    /// Enable PTY streaming sized to `terminal` when it is a real terminal.
    ///
    /// PTY support is not available on non-Unix targets, so this is always a
    /// no-op regardless of `terminal`: the runner keeps its deterministic
    /// pipe-backed capture for every unit.
    #[cfg(not(unix))]
    #[must_use]
    pub fn with_pty_matching_terminal<T>(self, _terminal: &T) -> Self {
        self
    }

    /// Render live-streamed commands on a pseudoterminal of the given size.
    ///
    /// When set, a unit whose output streams live (serial or single-unit runs)
    /// executes attached to a real terminal, so it renders exactly as it would
    /// interactively — colors, progress bars, and other tty-gated output are
    /// preserved. Buffered (parallel) units keep deterministic pipe capture.
    /// Leaving this unset keeps the pipe-backed behavior for every unit, which
    /// is the correct choice when output is redirected or captured (no tty).
    #[cfg(unix)]
    #[must_use]
    pub const fn with_pty(mut self, size: PtySize) -> Self {
        self.pty_size = Some(size);
        self
    }

    /// Override the process policy used for normal and persistent commands.
    #[must_use]
    pub fn with_process_config(mut self, config: ProcessConfig) -> Self {
        self.process_config = config;
        self
    }

    /// Drive spawned children through a caller-owned [`ProcessSupervisor`].
    ///
    /// By default the runner owns a private supervisor. Injecting a shared one
    /// lets a process-level shutdown handle (subscribed via
    /// [`ProcessSupervisor::subscribe_shutdown`]) reap every child this runner
    /// spawned as the backstop behind cooperative cancellation.
    #[must_use]
    pub fn with_supervisor(mut self, supervisor: Arc<ProcessSupervisor>) -> Self {
        self.supervisor = supervisor;
        self
    }

    /// The supervisor this runner registers spawned children with.
    ///
    /// A caller wires this into a shutdown handle so the supervisor reaps the
    /// runner's `cargo`/`nextest`/`rustc` groups on a process-level stop.
    #[must_use]
    pub fn supervisor(&self) -> Arc<ProcessSupervisor> {
        Arc::clone(&self.supervisor)
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
        let config = self
            .process_config
            .clone()
            .with_lifecycle_policy(invocation.lifecycle);
        let result = run_with_cancel_supervised(&self.supervisor, &spec, &config, cancel).await?;
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
        Ok(gate_outcome(
            invocation.fail_if_output,
            result.success(),
            result.exit_code,
            !result.stdout_bytes.is_empty(),
            output,
        ))
    }

    /// Stream stdout/stderr through `observer` as the process runs (no
    /// capture), returning an empty [`RunOutcome::output`] because the bytes
    /// were already surfaced live. Used when no two units can run concurrently.
    async fn run_streaming(
        &self,
        invocation: &Invocation,
        spec: ProcessSpec,
        cancel: CancellationToken,
        observer: OutputObserver,
    ) -> AppResult<RunOutcome> {
        let stdout_seen = Arc::new(AtomicBool::new(false));
        let io = ObservedIo::new(streaming_observer(
            invocation.unit_id.clone(),
            observer,
            Arc::clone(&stdout_seen),
        ))
        .with_output(OutputPolicy::observe_only());
        let config = self
            .process_config
            .clone()
            .with_io(ProcessIo::observed(io))
            .with_lifecycle_policy(invocation.lifecycle);
        let result = run_with_cancel_supervised(&self.supervisor, &spec, &config, cancel).await?;
        Ok(gate_outcome(
            invocation.fail_if_output,
            result.success(),
            result.exit_code,
            stdout_seen.load(Ordering::Relaxed),
            Vec::new(),
        ))
    }

    /// Stream stdout/stderr through a pseudoterminal so the child renders as it
    /// would in a real terminal, forwarding the merged stream through
    /// `observer`. Like [`Self::run_streaming`], output is surfaced live and
    /// the returned outcome carries no buffered bytes.
    #[cfg(unix)]
    async fn run_streaming_pty(
        &self,
        invocation: &Invocation,
        spec: ProcessSpec,
        cancel: CancellationToken,
        observer: OutputObserver,
        size: PtySize,
    ) -> AppResult<RunOutcome> {
        let stdout_seen = Arc::new(AtomicBool::new(false));
        let io = PtyIo::new(streaming_observer(
            invocation.unit_id.clone(),
            observer,
            Arc::clone(&stdout_seen),
        ))
        .with_size(size)
        .with_output(OutputPolicy::observe_only());
        let config = self
            .process_config
            .clone()
            .with_io(ProcessIo::pty(io))
            .with_lifecycle_policy(invocation.lifecycle);
        let result = run_with_cancel_supervised(&self.supervisor, &spec, &config, cancel).await?;
        Ok(gate_outcome(
            invocation.fail_if_output,
            result.success(),
            result.exit_code,
            stdout_seen.load(Ordering::Relaxed),
            Vec::new(),
        ))
    }

    /// Dispatch a live-streamed unit to the PTY renderer when one is configured
    /// (Unix), otherwise to the pipe-backed streamer.
    async fn run_live(
        &self,
        invocation: &Invocation,
        spec: ProcessSpec,
        cancel: CancellationToken,
        observer: OutputObserver,
    ) -> AppResult<RunOutcome> {
        #[cfg(unix)]
        if let Some(size) = self.pty_size {
            return self
                .run_streaming_pty(invocation, spec, cancel, observer, size)
                .await;
        }
        self.run_streaming(invocation, spec, cancel, observer).await
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
            Some(observer) => self.run_live(invocation, spec, cancel, observer).await,
            None => self.run_captured(invocation, spec, cancel).await,
        }
    }

    async fn start_persistent(
        &self,
        invocation: &Invocation,
        cancel: CancellationToken,
        output: OutputObserver,
    ) -> AppResult<StartOutcome> {
        start_persistent(
            invocation,
            &self.project_root,
            &self.process_config,
            Arc::clone(&self.supervisor),
            // Honor the caller's carried lifecycle intent: the persistent
            // teardown deadline is the invocation's own grace period, not a
            // separate runner-level knob that would silently shadow it.
            invocation.lifecycle.grace_period,
            cancel,
            output,
        )
        .await
    }
}

/// Lower an APPLY [`Invocation`] into a project-rooted [`ProcessSpec`].
///
/// The shared argv/env-policy lowering ([`base_spec`](super::spec::base_spec))
/// plus the APPLY-shape extra: the run's project root as the working
/// directory. Both the streaming runner and the persistent-spawn helper drive
/// through here so the argv guard and env-policy mapping exist in exactly one
/// place.
fn spec(invocation: &Invocation, project_root: &Path) -> AppResult<ProcessSpec> {
    Ok(
        super::spec::base_spec(&invocation.argv, &invocation.environment, "argv")?
            .dir(project_root),
    )
}

/// Start a persistent invocation and wait until readiness succeeds or fails.
async fn start_persistent(
    invocation: &Invocation,
    project_root: &Path,
    process_config: &ProcessConfig,
    supervisor: Arc<ProcessSupervisor>,
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
    let process_config = process_config
        .clone()
        .with_lifecycle_policy(invocation.lifecycle);
    // The supervised persistent process owns no supervisor of its own — its
    // registration lives in the injected supervisor's registry. Hand a clone of
    // that supervisor to the returned held handle so the registry (and thus the
    // still-running child) outlives the runner: dropping `ProcessCommandRunner`
    // before the caller shuts the handle down must not drain the registration
    // and reap the child the caller still owns.
    let handle_supervisor = Arc::clone(&supervisor);
    let run = tokio::task::spawn_blocking(move || {
        start_persistent_supervised(
            &supervisor,
            &spec,
            &process_config,
            &persistent_config,
            cancel,
        )
    })
    .await
    .map_err(AppError::internal)?;

    match run {
        Ok(run) => Ok(StartOutcome::Ready {
            output: Vec::new(),
            process: Box::new(ProcessHeldProcess {
                unit_id,
                process: Arc::new(Mutex::new(Some(run.process))),
                _supervisor: handle_supervisor,
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
    /// Keeps the injected supervisor's registry alive for as long as the caller
    /// holds this handle. A supervised persistent process owns no supervisor of
    /// its own, so without this the runner's supervisor could drop first, and
    /// its registry-drain backstop would reap the child out from under the
    /// still-held handle.
    _supervisor: Arc<ProcessSupervisor>,
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

fn readiness(invocation: &Invocation, project_root: &Path) -> AppResult<PersistentReadiness> {
    match &invocation.readiness {
        ExecutionReadiness::Started => Ok(PersistentReadiness::Started),
        ExecutionReadiness::OutputContains(value) => {
            Ok(PersistentReadiness::OutputContains(value.clone()))
        }
        ExecutionReadiness::Command(argv) => {
            // Run the readiness probe under the same explicit environment as the main
            // invocation so it inherits the task's PATH allowlist and vars; otherwise
            // common probe tools (`curl`, `sh`, …) may fail to spawn even when the
            // persistent command itself runs fine.
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

/// Resolve a run's final outcome, applying the `fail_if_output` gate.
///
/// A process that exited `0` but produced stdout is a failure when the unit set
/// `fail_if_output` (a list-mode verification such as `gofmt -l`, which reports
/// offenders on stdout yet always exits `0`). Every other case maps straight
/// from the process exit status.
const fn gate_outcome(
    fail_if_output: bool,
    success: bool,
    exit_code: Option<i32>,
    stdout_seen: bool,
    output: Vec<UnitOutput>,
) -> RunOutcome {
    if success && fail_if_output && stdout_seen {
        RunOutcome::failed(exit_code, output)
    } else if success {
        RunOutcome::succeeded(output)
    } else {
        RunOutcome::failed(exit_code, output)
    }
}

/// Build an `rskit-process` observer that forwards each raw stdout/stderr chunk
/// to `observer` as a [`UnitOutput`] tagged with `unit_id`, so the live bridge
/// streams it while the process is still running. Setting `stdout_seen` on the
/// first stdout chunk lets the `fail_if_output` gate observe output that was
/// streamed rather than captured.
fn streaming_observer(
    unit_id: String,
    observer: OutputObserver,
    stdout_seen: Arc<AtomicBool>,
) -> ProcessOutputObserver {
    let stdout_id = unit_id.clone();
    let stdout_sink = observer.clone();
    let stderr_id = unit_id;
    let stderr_sink = observer;
    ProcessOutputObserver::new()
        .with_stdout_bytes(move |bytes| {
            if !bytes.is_empty() {
                stdout_seen.store(true, Ordering::Relaxed);
            }
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

#[cfg(test)]
mod tests {
    use super::{ProcessCommandRunner, gate_outcome};

    #[test]
    fn runner_does_not_inherit_the_rskit_process_default_timeout() {
        // Regression: `ProcessConfig::default()` carries a 30s process timeout. Toven
        // owns the per-unit bound (unbounded unless `--timeout` is passed and enforced
        // cooperatively in the APPLY pool), so the runner must clear the inherited
        // default — otherwise any build/test unit longer than 30s is silently killed
        // mid-run.
        let runner = ProcessCommandRunner::new(".");
        assert!(
            runner.process_config.timeout.is_none(),
            "ProcessCommandRunner must not inherit rskit-process's default 30s timeout"
        );
    }

    #[test]
    fn gate_fails_a_zero_exit_that_emitted_output() {
        // `gofmt -l` lists offenders on stdout yet exits 0; with the gate on, that
        // stdout must turn the unit into a failure so CI catches unformatted code.
        let outcome = gate_outcome(true, true, Some(0), true, Vec::new());
        assert!(!outcome.success);
        assert_eq!(outcome.exit_code, Some(0));
    }

    #[test]
    fn gate_passes_a_zero_exit_with_no_output() {
        // Nothing to list means everything is formatted — a clean pass.
        let outcome = gate_outcome(true, true, Some(0), false, Vec::new());
        assert!(outcome.success);
    }

    #[test]
    fn gate_is_inert_without_the_flag() {
        // A normal unit that prints to stdout and exits 0 still succeeds.
        let outcome = gate_outcome(false, true, Some(0), true, Vec::new());
        assert!(outcome.success);
    }

    #[test]
    fn gate_preserves_a_genuine_nonzero_failure() {
        let outcome = gate_outcome(true, false, Some(2), true, Vec::new());
        assert!(!outcome.success);
        assert_eq!(outcome.exit_code, Some(2));
    }
}
