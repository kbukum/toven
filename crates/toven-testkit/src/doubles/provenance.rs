//! Shared [`ProvenancePhase`] double: [`FakeProvenancePhase`].
//!
//! Release-engine tests script a verification outcome and record the subjects
//! verified here instead of shelling to a real `gh attestation verify`. It is
//! `Clone` (shared state via `Arc<Mutex<…>>`) so a test can hold a recording
//! handle while the engine drives a boxed copy, can be scripted to report
//! whether an attestation exists (for the report-only preview), and can be
//! scripted to fail.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_ports::{ProvenanceOutcome, ProvenancePhase, ProvenanceSubject};

/// One verification call recorded by [`FakeProvenancePhase`].
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProvenanceCall {
    /// Working directory the verification ran in.
    pub root: PathBuf,
    /// The subjects the engine handed the phase, in order.
    pub subjects: Vec<ProvenanceSubject>,
}

#[derive(Debug, Clone)]
struct FakeProvenanceState {
    exists: bool,
    fail: Option<String>,
    calls: Vec<ProvenanceCall>,
}

/// A [`ProvenancePhase`] with a scripted outcome and call recording.
#[derive(Debug, Clone)]
pub struct FakeProvenancePhase {
    inner: Arc<Mutex<FakeProvenanceState>>,
}

impl Default for FakeProvenancePhase {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(FakeProvenanceState {
                exists: true,
                fail: None,
                calls: Vec::new(),
            })),
        }
    }
}

impl FakeProvenancePhase {
    /// A phase that verifies every subject successfully.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A phase that always fails with `message` (attestation tool missing), so
    /// the release aborts rather than claim a verification it never ran.
    #[must_use]
    pub fn failing(message: impl Into<String>) -> Self {
        let phase = Self::new();
        phase.state().fail = Some(message.into());
        phase
    }

    /// Script whether an attestation exists, as reported by
    /// `attestation_exists` (used by the report-only preview) and enforced by
    /// `verify`.
    #[must_use]
    pub fn with_existing(self, exists: bool) -> Self {
        self.state().exists = exists;
        self
    }

    /// Snapshot the recorded verification calls in call order.
    #[must_use]
    pub fn calls(&self) -> Vec<ProvenanceCall> {
        self.state().calls.clone()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, FakeProvenanceState> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl ProvenancePhase for FakeProvenancePhase {
    fn verify(&self, root: &Path, subjects: &[ProvenanceSubject]) -> AppResult<ProvenanceOutcome> {
        let (fail, exists) = {
            let mut state = self.state();
            state.calls.push(ProvenanceCall {
                root: root.to_path_buf(),
                subjects: subjects.to_vec(),
            });
            (state.fail.clone(), state.exists)
        };
        if let Some(message) = fail {
            return Err(AppError::new(ErrorCode::Internal, message));
        }
        if !exists {
            return Err(AppError::new(
                ErrorCode::Internal,
                "no build-provenance attestation found for a published subject",
            ));
        }
        Ok(ProvenanceOutcome::Verified)
    }

    fn attestation_exists(&self, _root: &Path, _subject: &ProvenanceSubject) -> AppResult<bool> {
        Ok(self.state().exists)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use toven_ports::{ProvenanceOutcome, ProvenancePhase, ProvenanceSubject};

    use super::FakeProvenancePhase;

    #[test]
    fn records_subjects_and_verifies() {
        let phase = FakeProvenancePhase::new();
        let subjects = vec![ProvenanceSubject::file(
            "toven.tar.gz",
            "sha256:abc",
            "dist/toven.tar.gz",
        )];
        let outcome = phase.verify(Path::new("/repo"), &subjects).expect("ok");
        assert_eq!(outcome, ProvenanceOutcome::Verified);
        let calls = phase.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].subjects, subjects);
    }

    #[test]
    fn missing_attestation_fails_verification() {
        let phase = FakeProvenancePhase::new().with_existing(false);
        let error = phase
            .verify(
                Path::new("/repo"),
                &[ProvenanceSubject::file("a", "sha256:1", "dist/a")],
            )
            .expect_err("fails");
        assert!(
            error
                .to_string()
                .contains("no build-provenance attestation")
        );
    }

    #[test]
    fn scripted_failure_surfaces_and_still_records() {
        let phase = FakeProvenancePhase::failing("gh boom");
        let error = phase
            .verify(
                Path::new("/repo"),
                &[ProvenanceSubject::file("a", "sha256:1", "dist/a")],
            )
            .expect_err("fails");
        assert!(error.to_string().contains("gh boom"));
        assert_eq!(phase.calls().len(), 1);
    }
}
