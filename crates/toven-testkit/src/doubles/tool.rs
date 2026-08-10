//! Shared [`ToolRunner`] double: [`FakeToolRunner`].
//!
//! One-shot tool tests configure a scripted exit classification and record the
//! argv-first [`ToolInvocation`]s the caller builds, instead of spawning a real
//! external tool. It is `Clone` and shares its recording state so an invocation
//! driven through a boxed trait object is observable from the handle a test
//! keeps.

use std::sync::{Arc, Mutex};

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_ports::{ToolInvocation, ToolOutcome, ToolRunner};

/// A [`ToolRunner`] with a scripted outcome and invocation recording.
#[derive(Debug, Clone)]
pub struct FakeToolRunner {
    inner: Arc<Mutex<FakeToolState>>,
}

#[derive(Debug, Clone)]
struct FakeToolState {
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
    timed_out: bool,
    cancelled: bool,
    fail: Option<String>,
    requests: Vec<ToolInvocation>,
    produced: Vec<(std::path::PathBuf, Vec<u8>)>,
}

impl Default for FakeToolRunner {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(FakeToolState {
                exit_code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
                timed_out: false,
                cancelled: false,
                fail: None,
                requests: Vec::new(),
                produced: Vec::new(),
            })),
        }
    }
}

impl FakeToolRunner {
    /// Construct a runner that reports a zero exit for every invocation.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Script the exit code the runner reports.
    #[must_use]
    pub fn with_exit_code(self, code: Option<i32>) -> Self {
        self.state().exit_code = code;
        self
    }

    /// Script the captured standard output the runner reports.
    #[must_use]
    pub fn with_stdout(self, stdout: impl Into<String>) -> Self {
        self.state().stdout = stdout.into();
        self
    }

    /// Script the captured standard error the runner reports.
    #[must_use]
    pub fn with_stderr(self, stderr: impl Into<String>) -> Self {
        self.state().stderr = stderr.into();
        self
    }

    /// Report the invocation as timed out.
    #[must_use]
    pub fn with_timed_out(self, timed_out: bool) -> Self {
        self.state().timed_out = timed_out;
        self
    }

    /// Report the invocation as cancelled.
    #[must_use]
    pub fn with_cancelled(self, cancelled: bool) -> Self {
        self.state().cancelled = cancelled;
        self
    }

    /// Make `run` fail with a typed spawn/IO error (an unspawnable tool).
    #[must_use]
    pub fn with_spawn_failure(self, message: impl Into<String>) -> Self {
        self.state().fail = Some(message.into());
        self
    }

    /// Have a successful `run` write `contents` to `path`, simulating the
    /// external tool producing an artifact (an archive, a signature) that the
    /// engine then normalizes back into its typed outcome. Parent directories
    /// are created; the file is written only on a zero-exit run.
    #[must_use]
    pub fn with_produced_file(self, path: impl Into<std::path::PathBuf>, contents: &[u8]) -> Self {
        self.state().produced.push((path.into(), contents.to_vec()));
        self
    }

    /// Snapshot the invocations the runner received, in call order.
    #[must_use]
    pub fn requests(&self) -> Vec<ToolInvocation> {
        self.state().requests.clone()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, FakeToolState> {
        self.inner.lock().expect("FakeToolRunner mutex poisoned")
    }
}

impl ToolRunner for FakeToolRunner {
    fn run(&self, invocation: &ToolInvocation) -> AppResult<ToolOutcome> {
        let mut state = self.state();
        state.requests.push(invocation.clone());
        if let Some(message) = &state.fail {
            return Err(AppError::new(ErrorCode::Internal, message.clone()));
        }
        // A tool that exits zero produces its declared artifacts, mirroring the
        // real tool writing archives/signatures the engine then normalizes.
        if matches!(state.exit_code, Some(0)) && !state.timed_out && !state.cancelled {
            for (path, contents) in state.produced.clone() {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|error| {
                        AppError::new(
                            ErrorCode::Internal,
                            format!("failed to create '{}': {error}", parent.display()),
                        )
                    })?;
                }
                std::fs::write(&path, &contents).map_err(|error| {
                    AppError::new(
                        ErrorCode::Internal,
                        format!(
                            "failed to write produced artifact '{}': {error}",
                            path.display()
                        ),
                    )
                })?;
            }
        }
        Ok(
            ToolOutcome::new(state.exit_code, state.stdout.clone(), state.stderr.clone())
                .timed_out_flag(state.timed_out)
                .cancelled_flag(state.cancelled),
        )
    }
}

#[cfg(test)]
mod tests {
    use toven_ports::{ToolInvocation, ToolRunner};

    use super::FakeToolRunner;

    #[test]
    fn records_requests_and_reports_the_scripted_outcome() {
        let runner = FakeToolRunner::new().with_exit_code(Some(2));
        let invocation = ToolInvocation::new(vec!["goreleaser".into(), "release".into()]);

        let outcome = runner.run(&invocation).expect("runs");

        assert!(!outcome.succeeded());
        assert_eq!(outcome.exit_code, Some(2));
        assert_eq!(runner.requests(), vec![invocation]);
    }

    #[test]
    fn a_spawn_failure_surfaces_as_a_typed_error() {
        let runner = FakeToolRunner::new().with_spawn_failure("goreleaser not found");
        let error = runner
            .run(&ToolInvocation::new(vec!["goreleaser".into()]))
            .expect_err("spawn failure surfaces");
        assert!(
            error.to_string().contains("goreleaser not found"),
            "{error}"
        );
    }

    #[test]
    fn a_zero_exit_writes_the_declared_produced_artifacts() {
        let dir = std::env::temp_dir().join(format!("toven-faketool-{}", std::process::id()));
        let artifact = dir.join("dist/app.tar.gz");
        let runner = FakeToolRunner::new().with_produced_file(&artifact, b"payload");

        runner
            .run(&ToolInvocation::new(vec!["goreleaser".into()]))
            .expect("runs");

        assert_eq!(
            std::fs::read(&artifact).expect("artifact written"),
            b"payload"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
