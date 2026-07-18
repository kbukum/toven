//! Shared [`ReleaseHost`] double: [`FakeReleaseHost`].
//!
//! Release-engine tests configure a scripted create-or-update outcome and record
//! every hosted-release call here instead of invoking a real forge CLI. It is
//! `Clone` so a test can hold a recording handle while the engine drives a boxed
//! copy.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_ports::{HostReleaseOutcome, HostedRelease, ReleaseHost};

/// One hosted-release call recorded by [`FakeReleaseHost`].
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct HostCall {
    /// Working directory the release was cut in.
    pub root: PathBuf,
    /// The hosted release the engine resolved.
    pub release: HostedRelease,
}

/// A [`ReleaseHost`] with a scripted outcome and call recording.
#[derive(Debug, Clone)]
pub struct FakeReleaseHost {
    inner: Arc<Mutex<FakeHostState>>,
}

#[derive(Debug, Clone)]
struct FakeHostState {
    outcome: HostReleaseOutcome,
    fail: Option<String>,
    calls: Vec<HostCall>,
}

impl Default for FakeReleaseHost {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(FakeHostState {
                outcome: HostReleaseOutcome::Created,
                fail: None,
                calls: Vec::new(),
            })),
        }
    }
}

impl FakeReleaseHost {
    /// Construct a host that reports every release as freshly created.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Script the outcome returned by `ensure_release` (e.g. an existing tag
    /// updated in place).
    #[must_use]
    pub fn with_outcome(self, outcome: HostReleaseOutcome) -> Self {
        self.state().outcome = outcome;
        self
    }

    /// Make `ensure_release` fail with a typed internal error.
    #[must_use]
    pub fn with_failure(self, message: impl Into<String>) -> Self {
        self.state().fail = Some(message.into());
        self
    }

    /// Snapshot the recorded hosted-release calls in call order.
    #[must_use]
    pub fn calls(&self) -> Vec<HostCall> {
        self.state().calls.clone()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, FakeHostState> {
        self.inner.lock().expect("FakeReleaseHost mutex poisoned")
    }
}

impl ReleaseHost for FakeReleaseHost {
    fn ensure_release(
        &self,
        root: &Path,
        release: &HostedRelease,
    ) -> AppResult<HostReleaseOutcome> {
        let mut state = self.state();
        state.calls.push(HostCall {
            root: root.to_path_buf(),
            release: release.clone(),
        });
        if let Some(message) = &state.fail {
            return Err(AppError::new(ErrorCode::Internal, message.clone()));
        }
        Ok(state.outcome)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use toven_ports::{HostReleaseOutcome, HostedRelease, ReleaseHost};

    use super::FakeReleaseHost;

    #[test]
    fn records_calls_and_returns_scripted_outcome() {
        let host = FakeReleaseHost::new().with_outcome(HostReleaseOutcome::Updated);
        let release = HostedRelease::new("rust/core@1.2.3", "core 1.2.3", "notes");

        let outcome = host
            .ensure_release(Path::new("/repo"), &release)
            .expect("ok");

        assert_eq!(outcome, HostReleaseOutcome::Updated);
        let calls = host.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].release.tag, "rust/core@1.2.3");
    }

    #[test]
    fn scripted_failure_surfaces_and_still_records() {
        let host = FakeReleaseHost::new().with_failure("gh boom");
        let release = HostedRelease::new("rust/core@1.2.3", "core 1.2.3", "notes");

        let error = host
            .ensure_release(Path::new("/repo"), &release)
            .expect_err("fails");

        assert!(error.to_string().contains("gh boom"));
        assert_eq!(host.calls().len(), 1);
    }
}
