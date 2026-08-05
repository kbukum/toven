//! The SLSA-provenance port — attest a build over exactly the approved,
//! published subjects (archive checksums and/or pushed image digests).
//!
//! The engine owns provenance *policy*: it collects the subjects that were
//! actually published (the archives' `SHA256SUMS` entries and any pushed image
//! digests) and hands the adapter exactly that set. This port is the thin
//! sliver: attest over the given subjects, or — read-only — report whether an
//! attestation already exists for one. Attesting exactly the published subjects
//! matches the immutable "attest what was actually published" rule.
//!
//! Implementations invoke their attestation tooling (`gh attestation`, an SLSA
//! generator) argv-only and read any forge token from the ambient environment
//! only — never logging it or placing it on argv.

use std::path::Path;

use rskit_errors::AppResult;

/// One subject a provenance attestation is cut over: a named artifact and its
/// `sha256:`-prefixed digest.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProvenanceSubject {
    /// The subject's display name (the artifact or image reference).
    pub name: String,
    /// The subject's digest, `sha256:`-prefixed (e.g. `sha256:abc...`).
    pub digest: String,
}

impl ProvenanceSubject {
    /// Construct a provenance subject from a `name` and its `digest`.
    #[must_use]
    pub fn new(name: impl Into<String>, digest: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            digest: digest.into(),
        }
    }
}

/// The outcome of attesting provenance over a set of subjects.
///
/// Provenance is immutable: an attestation is either newly cut or an identical
/// one already existed for every subject and was verified. A divergent existing
/// attestation is never edited — the adapter fails instead — so this enum has
/// no "updated" outcome.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub enum ProvenanceOutcome {
    /// No attestation existed for at least one subject, so one was cut.
    Attested,
    /// Every subject already carried a matching attestation (an idempotent
    /// re-run), so nothing was mutated.
    AlreadyComplete,
}

impl ProvenanceOutcome {
    /// Canonical wire/report name for the outcome.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Attested => "attested",
            Self::AlreadyComplete => "already-complete",
        }
    }
}

/// The SLSA-provenance port: attest a build over exactly the approved,
/// published subjects.
///
/// Object-safe so the engine can inject a `&dyn ProvenancePhase`.
/// Implementations attest over the given `subjects` and report whether an
/// attestation already exists for one.
pub trait ProvenancePhase: Send + Sync {
    /// Attest provenance over exactly `subjects` — the archives' `SHA256SUMS`
    /// entries and/or pushed image digests the engine resolved as actually
    /// published.
    ///
    /// # Errors
    /// Fails closed on an attestation-tool spawn or non-zero exit, and returns
    /// a typed conflict error when an existing attestation diverges from the
    /// intended one.
    fn attest(&self, root: &Path, subjects: &[ProvenanceSubject]) -> AppResult<ProvenanceOutcome>;

    /// Whether an attestation already exists for `subject` — a read-only forge
    /// query used by the mutation-free preview.
    ///
    /// # Errors
    /// Propagates an attestation-tool spawn/IO failure or a non-zero exit that
    /// is not the "attestation not found" signal.
    fn attestation_exists(&self, root: &Path, subject: &ProvenanceSubject) -> AppResult<bool>;
}

#[cfg(test)]
mod tests {
    use super::{ProvenanceOutcome, ProvenanceSubject};

    #[test]
    fn subject_carries_name_and_digest() {
        let subject = ProvenanceSubject::new("toven-x86_64.tar.gz", "sha256:abc");
        assert_eq!(subject.name, "toven-x86_64.tar.gz");
        assert_eq!(subject.digest, "sha256:abc");
    }

    #[test]
    fn outcome_names_are_stable() {
        assert_eq!(ProvenanceOutcome::Attested.as_str(), "attested");
        assert_eq!(
            ProvenanceOutcome::AlreadyComplete.as_str(),
            "already-complete"
        );
    }
}
