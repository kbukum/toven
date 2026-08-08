//! The SLSA-provenance port — verify that exactly the approved, published
//! subjects (archive checksums and/or pushed image digests) carry a build
//! provenance attestation.
//!
//! Toven does not *create* attestations: build provenance is cut by the CI
//! workflow's trusted builder (`actions/attest-build-provenance`) over the
//! published `SHA256SUMS` subjects. The engine owns provenance *policy* — it
//! collects the subjects that were actually published (the archives'
//! `SHA256SUMS` entries and any pushed image digests) and hands the adapter
//! exactly that set. This port is the thin sliver: verify that every subject
//! carries an attestation, or — for the preview — report whether one exists for
//! a single subject. Verifying exactly the published subjects matches the
//! "attest what was actually published" rule.
//!
//! Implementations invoke their attestation tooling (`gh attestation verify`)
//! argv-only and read any forge token from the ambient environment only — never
//! logging it or placing it on argv.

use std::path::Path;

use rskit_errors::AppResult;

/// How a [`ProvenanceSubject`] is located so its attestation can be verified.
///
/// `gh attestation verify` resolves an artifact by reading its bytes (a file
/// path) or by its OCI reference — not by a bare digest — so the subject
/// carries the verifiable locator alongside its digest. Every published subject
/// is one or the other, so this distinction is exhaustive.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ProvenanceArtifact {
    /// A file on disk, verified by its project-relative path.
    File(String),
    /// A pushed image, verified by its OCI reference (without the `oci://`
    /// scheme prefix).
    Image(String),
}

/// One subject whose build provenance is verified: a named artifact, its
/// `sha256:`-prefixed digest, and how to locate it for verification.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProvenanceSubject {
    /// The subject's display name (the artifact file name or image reference).
    pub name: String,
    /// The subject's digest, `sha256:`-prefixed (e.g. `sha256:abc...`).
    pub digest: String,
    /// How to locate the subject so its attestation can be verified.
    pub artifact: ProvenanceArtifact,
}

impl ProvenanceSubject {
    /// A file subject verified by its project-relative `path`.
    #[must_use]
    pub fn file(
        name: impl Into<String>,
        digest: impl Into<String>,
        path: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            digest: digest.into(),
            artifact: ProvenanceArtifact::File(path.into()),
        }
    }

    /// An image subject verified by its OCI `reference`, which is also its
    /// display name.
    #[must_use]
    pub fn image(reference: impl Into<String>, digest: impl Into<String>) -> Self {
        let reference = reference.into();
        Self {
            name: reference.clone(),
            digest: digest.into(),
            artifact: ProvenanceArtifact::Image(reference),
        }
    }
}

/// The outcome of verifying provenance over a set of subjects.
///
/// Verification either succeeds — every subject carries a matching attestation
/// — or fails closed with a typed error; there is no partial outcome.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub enum ProvenanceOutcome {
    /// Every subject carries a matching build-provenance attestation.
    Verified,
}

impl ProvenanceOutcome {
    /// Canonical wire/report name for the outcome.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "verified",
        }
    }
}

/// The SLSA-provenance port: verify that exactly the approved, published
/// subjects carry a build-provenance attestation.
///
/// Object-safe so the engine can inject a `&dyn ProvenancePhase`.
/// Implementations verify the given `subjects` and report whether an
/// attestation exists for one.
pub trait ProvenancePhase: Send + Sync {
    /// Verify that every subject in `subjects` — the archives' `SHA256SUMS`
    /// entries and/or pushed image digests the engine resolved as actually
    /// published — carries a build-provenance attestation.
    ///
    /// # Errors
    /// Fails closed on an attestation-tool spawn or non-zero exit, and returns
    /// a typed error when any subject lacks an attestation.
    fn verify(&self, root: &Path, subjects: &[ProvenanceSubject]) -> AppResult<ProvenanceOutcome>;

    /// Whether an attestation exists for `subject` — a read-only forge query
    /// used by the report-only preview.
    ///
    /// # Errors
    /// Propagates an attestation-tool spawn/IO failure or a non-zero exit that
    /// is not the "attestation not found" signal.
    fn attestation_exists(&self, root: &Path, subject: &ProvenanceSubject) -> AppResult<bool>;
}

#[cfg(test)]
mod tests {
    use super::{ProvenanceArtifact, ProvenanceOutcome, ProvenanceSubject};

    #[test]
    fn file_subject_carries_name_digest_and_path() {
        let subject = ProvenanceSubject::file(
            "toven-x86_64.tar.gz",
            "sha256:abc",
            "dist/toven-x86_64.tar.gz",
        );
        assert_eq!(subject.name, "toven-x86_64.tar.gz");
        assert_eq!(subject.digest, "sha256:abc");
        assert_eq!(
            subject.artifact,
            ProvenanceArtifact::File("dist/toven-x86_64.tar.gz".to_string())
        );
    }

    #[test]
    fn image_subject_reuses_the_reference_as_its_name() {
        let subject = ProvenanceSubject::image("ghcr.io/acme/toven:1.0.0", "sha256:def");
        assert_eq!(subject.name, "ghcr.io/acme/toven:1.0.0");
        assert_eq!(
            subject.artifact,
            ProvenanceArtifact::Image("ghcr.io/acme/toven:1.0.0".to_string())
        );
    }

    #[test]
    fn outcome_names_are_stable() {
        assert_eq!(ProvenanceOutcome::Verified.as_str(), "verified");
    }
}
