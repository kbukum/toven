//! Shared [`DelegatedPhase`] double: [`FakeDelegatedPhase`].
//!
//! Delegation tests configure a scripted exit classification and record the
//! argv-first requests the engine builds, instead of spawning a real external
//! tool. It is `Clone` and shares its recording state so a request driven
//! through a boxed trait object is observable from the handle a test keeps.

use std::sync::{Arc, Mutex};

use rskit_errors::AppResult;
use toven_ports::{DelegatedPhase, DelegatedPhaseOutcome, DelegatedPhaseRequest};

/// A [`DelegatedPhase`] with a scripted outcome and request recording.
#[derive(Debug, Clone)]
pub struct FakeDelegatedPhase {
    inner: Arc<Mutex<FakeDelegatedState>>,
}

#[derive(Debug, Clone)]
struct FakeDelegatedState {
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
    fail: Option<String>,
    requests: Vec<DelegatedPhaseRequest>,
    produced: Vec<(std::path::PathBuf, Vec<u8>)>,
}

impl Default for FakeDelegatedPhase {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(FakeDelegatedState {
                exit_code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
                fail: None,
                requests: Vec::new(),
                produced: Vec::new(),
            })),
        }
    }
}

impl FakeDelegatedPhase {
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

    /// Snapshot the requests the runner received, in call order.
    #[must_use]
    pub fn requests(&self) -> Vec<DelegatedPhaseRequest> {
        self.state().requests.clone()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, FakeDelegatedState> {
        self.inner
            .lock()
            .expect("FakeDelegatedPhase mutex poisoned")
    }
}

impl DelegatedPhase for FakeDelegatedPhase {
    fn run(&self, request: &DelegatedPhaseRequest) -> AppResult<DelegatedPhaseOutcome> {
        let mut state = self.state();
        state.requests.push(request.clone());
        if let Some(message) = &state.fail {
            return Err(rskit_errors::AppError::new(
                rskit_errors::ErrorCode::Internal,
                message.clone(),
            ));
        }
        // A tool that exits zero produces its declared artifacts, mirroring the
        // real tool writing archives/signatures the engine then normalizes.
        if matches!(state.exit_code, Some(0)) {
            for (path, contents) in state.produced.clone() {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|error| {
                        rskit_errors::AppError::new(
                            rskit_errors::ErrorCode::Internal,
                            format!("failed to create '{}': {error}", parent.display()),
                        )
                    })?;
                }
                std::fs::write(&path, &contents).map_err(|error| {
                    rskit_errors::AppError::new(
                        rskit_errors::ErrorCode::Internal,
                        format!(
                            "failed to write produced artifact '{}': {error}",
                            path.display()
                        ),
                    )
                })?;
            }
        }
        Ok(DelegatedPhaseOutcome::new(
            state.exit_code,
            state.stdout.clone(),
            state.stderr.clone(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use toven_model::ReleasePhase;
    use toven_ports::{DelegatedPhase, DelegatedPhaseMode, DelegatedPhaseRequest};

    use super::FakeDelegatedPhase;

    #[test]
    fn records_requests_and_reports_the_scripted_outcome() {
        let runner = FakeDelegatedPhase::new().with_exit_code(Some(2));
        let request = DelegatedPhaseRequest::new(
            ReleasePhase::Package,
            vec!["goreleaser".into(), "release".into()],
            DelegatedPhaseMode::Apply,
            "/repo",
        );

        let outcome = runner.run(&request).expect("runs");

        assert!(!outcome.succeeded());
        assert_eq!(outcome.exit_code, Some(2));
        assert_eq!(runner.requests(), vec![request]);
    }

    #[test]
    fn a_spawn_failure_surfaces_as_a_typed_error() {
        let runner = FakeDelegatedPhase::new().with_spawn_failure("goreleaser not found");
        let error = runner
            .run(&DelegatedPhaseRequest::new(
                ReleasePhase::Package,
                vec!["goreleaser".into()],
                DelegatedPhaseMode::Preview,
                "/repo",
            ))
            .expect_err("spawn failure surfaces");
        assert!(
            error.to_string().contains("goreleaser not found"),
            "{error}"
        );
    }
}
