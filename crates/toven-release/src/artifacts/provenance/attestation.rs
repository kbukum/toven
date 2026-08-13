//! `gh attestation` provenance adapter.

use std::path::Path;
use std::sync::Arc;

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::safe_join;
use rskit_fs::sync_io::file::open;
use rskit_util::hash::sha256::sha256_reader;
use toven_ports::{
    ProvenanceArtifact, ProvenanceOutcome, ProvenancePhase, ProvenanceSubject, ToolInvocation,
    ToolOutcome, ToolRunner,
};

/// The trusted-builder workflow whose attestations provenance verification will
/// accept, as a repository-relative path. `--repo` alone lets any workflow in
/// the repository satisfy the check; binding `--signer-workflow` to this
/// workflow requires the attestation to have been cut by that specific builder.
/// `gh` expects an `<owner>/<repo>/<path>` value (matched against the signing
/// certificate SAN), so the resolved repository slug is prefixed at argv-build
/// time — see [`verify_argv`].
const TRUSTED_SIGNER_WORKFLOW: &str = ".github/workflows/release.yml";

/// Hard bound on captured tool output (256 KiB).
const MAX_PROVENANCE_OUTPUT_BYTES: usize = 256 * 1024;

/// Timeout for a single attestation invocation.
const PROVENANCE_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(10);

/// A `gh attestation`-backed [`ProvenancePhase`].
///
/// Construction injects the shared [`ToolRunner`] plus a lazily-resolved
/// repository slug. `gh` is invoked argv-only through the runner; the ambient
/// forge token the runner provides is inherited from the environment, and no
/// secret is placed on argv or captured. `gh attestation verify` requires an
/// explicit `--owner`/`--repo` (unlike `gh release`, it does not infer the
/// repository from the working directory), so the adapter resolves the
/// `owner/name` slug once via `gh repo view` and caches it.
#[derive(Clone)]
pub struct GhAttestationProvenance {
    runner: Arc<dyn ToolRunner>,
    timeout: std::time::Duration,
    repo: std::sync::Arc<std::sync::OnceLock<String>>,
}

impl GhAttestationProvenance {
    /// Construct a `gh attestation` provenance phase driven through `runner`
    /// with the default timeout.
    #[must_use]
    pub fn new(runner: Arc<dyn ToolRunner>) -> Self {
        Self {
            runner,
            timeout: PROVENANCE_TIMEOUT,
            repo: std::sync::Arc::new(std::sync::OnceLock::new()),
        }
    }

    /// The `owner/name` slug of the repository `root` belongs to, resolved once
    /// via `gh repo view` and cached. Required for `gh attestation verify`.
    fn repo_slug(&self, root: &Path) -> AppResult<String> {
        if let Some(slug) = self.repo.get() {
            return Ok(slug.clone());
        }
        let outcome = self.run(root, repo_view_argv())?;
        if !outcome.succeeded() {
            return Err(process_failure("release.provenance.repo", "gh", &outcome));
        }
        let slug = outcome.stdout.trim().to_string();
        if slug.is_empty() {
            return Err(AppError::new(
                ErrorCode::Internal,
                "gh repo view returned no repository slug for provenance verification",
            )
            .with_detail("field", "release.provenance.repo"));
        }
        let _ = self.repo.set(slug.clone());
        Ok(slug)
    }

    /// Whether an attestation exists for `subject` in `repo`, classifying the
    /// `gh attestation verify` result explicitly. A file subject's on-disk bytes
    /// are hashed and compared to the manifest digest first, so a file that no
    /// longer matches the digest Toven reported fails closed before `gh` (which
    /// would otherwise silently re-hash and attest whatever is on disk).
    fn attestation_exists_in(
        &self,
        root: &Path,
        subject: &ProvenanceSubject,
        repo: &str,
    ) -> AppResult<bool> {
        ensure_file_matches_digest(root, subject)?;
        let outcome = self.run(root, verify_argv(subject, repo))?;
        if outcome.succeeded() {
            return Ok(true);
        }
        if attestation_not_found(&outcome) {
            return Ok(false);
        }
        Err(process_failure(
            "release.provenance.attestation",
            "gh",
            &outcome,
        ))
    }

    /// Run an argv-only `gh` invocation rooted at `root` through the shared
    /// runner, returning its classified outcome for explicit classification.
    fn run(&self, root: &Path, argv: Vec<String>) -> AppResult<ToolOutcome> {
        let mut full_argv = Vec::with_capacity(argv.len() + 1);
        full_argv.push("gh".to_string());
        full_argv.extend(argv);
        let invocation = ToolInvocation::new(full_argv)
            .with_working_dir(root)
            .with_timeout(self.timeout)
            .with_max_output_bytes(MAX_PROVENANCE_OUTPUT_BYTES);
        self.runner.run(&invocation)
    }
}

impl ProvenancePhase for GhAttestationProvenance {
    fn verify(&self, root: &Path, subjects: &[ProvenanceSubject]) -> AppResult<ProvenanceOutcome> {
        if subjects.is_empty() {
            return Err(AppError::invalid_input(
                "release.provenance.subjects",
                "no subjects to verify",
            ));
        }
        let repo = self.repo_slug(root)?;
        for subject in subjects {
            if !self.attestation_exists_in(root, subject, &repo)? {
                return Err(AppError::new(
                    ErrorCode::Internal,
                    format!(
                        "no build-provenance attestation found for published subject '{}'",
                        subject.name
                    ),
                )
                .with_detail("field", "release.provenance.subject")
                .with_detail("subject", subject.name.clone()));
            }
        }
        Ok(ProvenanceOutcome::Verified)
    }

    fn attestation_exists(&self, root: &Path, subject: &ProvenanceSubject) -> AppResult<bool> {
        let repo = self.repo_slug(root)?;
        self.attestation_exists_in(root, subject, &repo)
    }
}

/// Whether `gh attestation verify` reported the specific absence condition that
/// the verification and preview treat as "no attestation for this subject". A
/// current `gh` (>= 2.67.0) surfaces "no attestation exists for this digest" as
/// an HTTP 404 from the repository attestations lookup endpoint
/// (`/repos/<owner>/<repo>/attestations/<digest>`) and exits non-zero; older
/// builds print "no attestations found" instead. Both are the same absence
/// signal. Because the repository slug is resolved via `gh repo view` before any
/// verification runs, accessibility is already established, so a 404 *on the
/// attestations endpoint* is the digest being unattested — not an inaccessible
/// repository. Every other non-zero result — auth/token errors, network
/// failures, malformed argv, a 404 that never reached the attestations endpoint
/// — fails closed rather than being misread as a benignly absent attestation.
pub(super) fn attestation_not_found(outcome: &ToolOutcome) -> bool {
    let output = format!("{}\n{}", outcome.stdout, outcome.stderr).to_ascii_lowercase();
    output.contains("no attestations found")
        || output.contains("no attestation found")
        || attestations_endpoint_not_found(&output)
}

/// Whether the combined tool output is an HTTP 404 from the repository
/// attestations lookup endpoint, which `gh attestation verify` returns when no
/// attestation exists for the queried digest. Matching the `/attestations/`
/// path keeps this distinct from an auth/repository 404 that never reached the
/// attestations lookup.
fn attestations_endpoint_not_found(lowercased_output: &str) -> bool {
    lowercased_output.contains("404") && lowercased_output.contains("/attestations/")
}

/// Convert a non-zero tool outcome into a typed fail-closed error with bounded
/// captured diagnostics.
fn process_failure(field: &str, program: &str, outcome: &ToolOutcome) -> AppError {
    let mut message = format!(
        "{program} exited with code {}; refusing to treat the result as absent",
        outcome
            .exit_code
            .map_or_else(|| "unknown".to_string(), |code| code.to_string())
    );
    let stderr = outcome.stderr.trim();
    if !stderr.is_empty() {
        message.push_str(": ");
        message.push_str(stderr);
    }
    AppError::new(ErrorCode::Internal, message)
        .with_detail("field", field)
        .with_detail("program", program)
}

/// Build the read-only `gh repo view` invocation that resolves the working
/// directory's repository slug (`owner/name`) for `--repo` on
/// `gh attestation verify`.
pub(super) fn repo_view_argv() -> Vec<String> {
    vec![
        "repo".to_string(),
        "view".to_string(),
        "--json".to_string(),
        "nameWithOwner".to_string(),
        "--jq".to_string(),
        ".nameWithOwner".to_string(),
    ]
}

/// Build the argv-only `gh attestation verify` invocation that checks whether an
/// attestation exists for `subject` in `repo`. A file subject is verified by its
/// project-relative path (resolved against the working directory the command
/// runs in); an image subject by its digest-pinned `oci://` reference, so the
/// registry cannot resolve the tag to a different digest than the one Toven
/// collected. Verification is bound to the trusted builder via
/// `--signer-workflow <owner>/<repo>/.github/workflows/release.yml` (the
/// repository-qualified form `gh` matches against the signing certificate SAN),
/// so an attestation cut by any other workflow in the same repository does not
/// satisfy the check.
pub(super) fn verify_argv(subject: &ProvenanceSubject, repo: &str) -> Vec<String> {
    let target = match &subject.artifact {
        ProvenanceArtifact::File(path) => path.clone(),
        ProvenanceArtifact::Image(reference) => {
            format!("oci://{reference}@{}", subject.digest)
        }
    };
    vec![
        "attestation".to_string(),
        "verify".to_string(),
        target,
        "--repo".to_string(),
        repo.to_string(),
        "--signer-workflow".to_string(),
        format!("{repo}/{TRUSTED_SIGNER_WORKFLOW}"),
    ]
}

/// Hash a file subject's on-disk bytes and require them to match the manifest
/// digest Toven collected, failing closed on any mismatch. Image subjects carry
/// their digest in the pinned `oci://` reference instead, so this is a no-op for
/// them.
///
/// # Errors
/// Fails closed when the file cannot be read or its computed sha256 differs from
/// the manifest digest.
pub(super) fn ensure_file_matches_digest(
    root: &Path,
    subject: &ProvenanceSubject,
) -> AppResult<()> {
    let ProvenanceArtifact::File(rel) = &subject.artifact else {
        return Ok(());
    };
    let path = safe_join(root, rel).map_err(|error| {
        AppError::invalid_input(
            "release.provenance.subject",
            format!("subject '{rel}' is not a safe project-relative path"),
        )
        .with_cause(error)
    })?;
    let mut file = open(&path).map_err(|error| {
        AppError::new(
            ErrorCode::Internal,
            format!("cannot open subject '{rel}' to verify its digest: {error}"),
        )
        .with_cause(error)
    })?;
    let actual = sha256_reader(&mut file)?.to_hex();
    let expected = subject
        .digest
        .strip_prefix("sha256:")
        .unwrap_or(&subject.digest);
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(AppError::new(
            ErrorCode::Internal,
            format!(
                "subject '{rel}' on disk has digest sha256:{actual} but the manifest declares \
                 sha256:{expected}; refusing to verify a file that does not match what was reported"
            ),
        )
        .with_detail("field", "release.provenance.subject")
        .with_detail("subject", subject.name.clone()));
    }
    Ok(())
}
