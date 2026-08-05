//! Shared [`ProvenancePhase`] double: [`FakeProvenancePhase`].
//!
//! Release-engine tests script an attestation outcome and record the subjects
//! attested here instead of shelling to a real `gh attestation`/SLSA
//! generator. It is `Clone` (shared state via `Arc<Mutex<…>>`) so a test can
//! hold a recording handle while the engine drives a boxed copy, can be
//! scripted to report an existing attestation (for the mutation-free preview),
//! and can be scripted to fail.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_ports::{ProvenanceOutcome, ProvenancePhase, ProvenanceSubject};

/// One attestation call recorded by [`FakeProvenancePhase`].
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProvenanceCall {
    /// Working directory the attestation was cut in.
    pub root: PathBuf,
    /// The subjects the engine handed the phase, in order.
    pub subjects: Vec<ProvenanceSubject>,
}

#[derive(Debug, Clone)]
struct FakeProvenanceState {
    outcome: ProvenanceOutcome,
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
                outcome: ProvenanceOutcome::Attested,
                exists: false,
                fail: None,
                calls: Vec::new(),
            })),
        }
    }
}

impl FakeProvenancePhase {
    /// A phase that attests every subject freshly.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A phase that always fails with `message` (attestation tool missing), so
    /// the release aborts rather than claim an attestation it never cut.
    #[must_use]
    pub fn failing(message: impl Into<String>) -> Self {
        let phase = Self::new();
        phase.state().fail = Some(message.into());
        phase
    }

    /// Script the attestation outcome (e.g. an idempotent re-run).
    #[must_use]
    pub fn with_outcome(self, outcome: ProvenanceOutcome) -> Self {
        self.state().outcome = outcome;
        self
    }

    /// Script whether an attestation already exists, as reported by
    /// `attestation_exists` (used by the mutation-free preview).
    #[must_use]
    pub fn with_existing(self, exists: bool) -> Self {
        self.state().exists = exists;
        self
    }

    /// Snapshot the recorded attestation calls in call order.
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
    fn attest(&self, root: &Path, subjects: &[ProvenanceSubject]) -> AppResult<ProvenanceOutcome> {
        let mut state = self.state();
        state.calls.push(ProvenanceCall {
            root: root.to_path_buf(),
            subjects: subjects.to_vec(),
        });
        if let Some(message) = &state.fail {
            return Err(AppError::new(ErrorCode::Internal, message.clone()));
        }
        Ok(state.outcome)
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
    fn records_subjects_and_returns_scripted_outcome() {
        let phase = FakeProvenancePhase::new().with_outcome(ProvenanceOutcome::AlreadyComplete);
        let subjects = vec![ProvenanceSubject::new("toven.tar.gz", "sha256:abc")];
        let outcome = phase.attest(Path::new("/repo"), &subjects).expect("ok");
        assert_eq!(outcome, ProvenanceOutcome::AlreadyComplete);
        let calls = phase.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].subjects, subjects);
    }

    #[test]
    fn scripted_failure_surfaces_and_still_records() {
        let phase = FakeProvenancePhase::failing("gh boom");
        let error = phase
            .attest(
                Path::new("/repo"),
                &[ProvenanceSubject::new("a", "sha256:1")],
            )
            .expect_err("fails");
        assert!(error.to_string().contains("gh boom"));
        assert_eq!(phase.calls().len(), 1);
    }
}
