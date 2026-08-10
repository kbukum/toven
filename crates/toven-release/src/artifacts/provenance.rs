//! `release provenance` verb and the `gh attestation` [`ProvenancePhase`]
//! adapter.
//!
//! Toven does not *create* attestations: build provenance is cut by the CI
//! workflow's trusted builder (`actions/attest-build-provenance`) over the
//! published `SHA256SUMS` subjects. The engine owns provenance *policy*: which
//! subjects an attestation must cover. Those subjects are exactly what was
//! actually published — the entries of the declared `SHA256SUMS` manifest (each
//! archive/SBOM and its digest) and the live digest of every pushed image
//! reference — so verification covers the released bytes and nothing else. The
//! only reusable primitive is "run a subprocess" (the shared [`ToolRunner`]);
//! [`GhAttestationProvenance`] shells to `gh attestation verify` argv-only,
//! reading the ambient forge token from the environment — it embeds no secret
//! and captures none.
//!
//! Verification is read-only: `release provenance` asserts that every published
//! subject carries an attestation and fails closed if any is missing, while its
//! `--dry-run` preview only reports presence and never fails.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::safe_join;
use rskit_fs::sync_io::file::read_string_bounded;
use rskit_util::hash::sha256::sha256_reader;
use toven_ports::{
    ProvenanceArtifact, ProvenanceOutcome, ProvenancePhase, ProvenanceSubject, Provider, Reporter,
    ToolInvocation, ToolOutcome, ToolRunner,
};

use crate::planning::plan::{release_targets, resolve_release_settings};
use toven_core::config::Document;
use toven_core::federation::resolve::PathDriverLocator;
use toven_core::plan::{PlanRequest, prepare_front};

/// The canonical file name of the checksum manifest whose entries are the
/// provenance subjects. A declared asset whose file name is exactly this is the
/// manifest; one beginning `SHA256SUMS.` is a signature sidecar, not a subject.
const MANIFEST_NAME: &str = "SHA256SUMS";

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

/// Hard bound on the untrusted `SHA256SUMS` manifest read (1 MiB) — a checksum
/// line is ~80 bytes, so this admits thousands of assets while rejecting a
/// pathological file.
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

/// Timeout for a single attestation invocation.
const PROVENANCE_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(10);

/// The resolved status of the provenance phase over the release scope.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub enum ProvenancePhaseStatus {
    /// An enforced run: every published subject carries an attestation.
    Verified,
    /// `--dry-run`: every subject already carries an attestation.
    Present,
    /// `--dry-run`: at least one subject lacks an attestation.
    Missing,
}

impl ProvenancePhaseStatus {
    /// Canonical wire/report name for the status.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Present => "present",
            Self::Missing => "missing",
        }
    }
}

/// A single subject's provenance result within a [`ProvenanceReport`].
///
/// Each subject carries its own resolved [`ProvenancePhaseStatus`] so that a
/// `--dry-run` preview reports `present`/`missing` **per subject** — an attested
/// subject is never masked by an unattested sibling. In an enforced run every
/// listed subject is `Verified` (the phase fails closed before producing a
/// report when any subject is missing).
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProvenanceSubjectReport {
    /// The published subject this result covers.
    pub subject: ProvenanceSubject,
    /// This subject's resolved status.
    pub status: ProvenancePhaseStatus,
}

/// A read-only projection of the provenance phase over the release scope.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProvenanceReport {
    /// Whether this report is a mutation-free `--dry-run` preview.
    pub preview: bool,
    /// The per-subject results, in manifest order.
    pub subjects: Vec<ProvenanceSubjectReport>,
    /// The aggregate status of the phase.
    pub status: ProvenancePhaseStatus,
}

/// Options controlling the provenance phase.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProvenanceOptions {
    /// Preview the phase in report-only mode: report whether an attestation
    /// exists for each subject without failing when one is missing.
    pub dry_run: bool,
}

/// Verify SLSA provenance over exactly the published subjects.
///
/// The subjects are the entries of the declared `SHA256SUMS` manifest plus the
/// live digest of every pushed image reference. Toven never cuts the
/// attestation itself (the CI trusted builder does); this asserts that every
/// published subject carries one.
///
/// With `options.dry_run`, the phase is a report-only preview: it reports
/// whether an attestation exists for each subject but never fails on a missing
/// one. Otherwise it verifies every subject and fails closed if any is missing.
///
/// # Errors
/// Fails closed with a typed error when neither a `SHA256SUMS` manifest nor a
/// published image is available, the manifest file is missing or malformed,
/// nothing resolves to a subject, or (outside `--dry-run`) any subject lacks an
/// attestation — and propagates configuration/discovery/graph failures and
/// attestation-tool failures.
pub fn release_provenance(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    provenance_phase: &dyn ProvenancePhase,
    image_phase: &dyn toven_ports::ImagePhase,
    options: ProvenanceOptions,
    reporter: &mut dyn Reporter,
) -> AppResult<ProvenanceReport> {
    let locator = PathDriverLocator::new();
    let context = prepare_front(
        &request.project_root,
        document,
        providers,
        &locator,
        reporter,
    )?;
    let targets = release_targets(&context)?;
    let settings = resolve_release_settings(&context, &targets)?;
    let project_root = request.project_root.as_path();

    let image_requests = super::image::resolved_image_requests(&context, &targets, &settings)?;
    let subjects = published_subjects(project_root, &settings, &image_requests, image_phase)?;

    let (subject_reports, status) = if options.dry_run {
        let mut reports = Vec::with_capacity(subjects.len());
        let mut all_present = true;
        for subject in subjects {
            let present = provenance_phase.attestation_exists(project_root, &subject)?;
            if !present {
                all_present = false;
            }
            let status = if present {
                ProvenancePhaseStatus::Present
            } else {
                ProvenancePhaseStatus::Missing
            };
            reports.push(ProvenanceSubjectReport { subject, status });
        }
        let aggregate = if all_present {
            ProvenancePhaseStatus::Present
        } else {
            ProvenancePhaseStatus::Missing
        };
        (reports, aggregate)
    } else {
        provenance_phase.verify(project_root, &subjects)?;
        let reports = subjects
            .into_iter()
            .map(|subject| ProvenanceSubjectReport {
                subject,
                status: ProvenancePhaseStatus::Verified,
            })
            .collect();
        (reports, ProvenancePhaseStatus::Verified)
    };

    Ok(ProvenanceReport {
        preview: options.dry_run,
        subjects: subject_reports,
        status,
    })
}

/// Collect the published subjects: the entries of the declared `SHA256SUMS`
/// manifest (each a `sha256:`-prefixed subject, in manifest order) plus the live
/// digest of every published image reference. Provenance attests exactly what
/// was actually published, so an image that has not been pushed contributes no
/// subject.
///
/// # Errors
/// Fails closed when neither a manifest nor an image block is declared, when a
/// declared manifest is missing/malformed or lists no subjects, or when nothing
/// resolves to a subject at all.
fn published_subjects(
    project_root: &Path,
    settings: &BTreeMap<toven_model::ModuleKey, crate::ResolvedReleaseSettings>,
    image_requests: &[(String, toven_ports::ImageRequest)],
    image_phase: &dyn toven_ports::ImagePhase,
) -> AppResult<Vec<ProvenanceSubject>> {
    let declared = super::assets::declared_release_assets(settings);
    let manifest = declared
        .iter()
        .find(|asset| asset_file_name(asset) == Some(MANIFEST_NAME))
        .copied();

    let mut subjects = Vec::new();
    if let Some(manifest) = manifest {
        let path = safe_join(project_root, manifest).map_err(|error| {
            AppError::invalid_input(
                "release.host.assets",
                format!("asset '{manifest}' is not a safe project-relative path"),
            )
            .with_cause(error)
        })?;
        let manifest_subjects = parse_manifest_subjects(manifest, &path)?;
        if manifest_subjects.is_empty() {
            return Err(AppError::invalid_input(
                "release.provenance.subjects",
                format!("manifest '{manifest}' lists no subjects to verify"),
            ));
        }
        subjects.extend(manifest_subjects);
    } else if image_requests.is_empty() {
        return Err(AppError::invalid_input(
            "release.provenance.subjects",
            format!(
                "no '{MANIFEST_NAME}' manifest and no image are declared; provenance verifies \
                 exactly the published subjects (manifest entries and pushed image digests)"
            ),
        ));
    }

    subjects.extend(image_subjects(project_root, image_requests, image_phase)?);

    if subjects.is_empty() {
        return Err(AppError::invalid_input(
            "release.provenance.subjects",
            "nothing published to verify: the declared image references resolve to no pushed \
             digest — run `toven release image` first",
        ));
    }
    Ok(subjects)
}

/// The live digest of each published primary image reference, as a provenance
/// subject. A primary reference the registry does not yet resolve (the image was
/// not pushed) contributes nothing — provenance attests only what was actually
/// published by the authoritative registry. Mirrors never substitute for a
/// missing primary.
fn image_subjects(
    project_root: &Path,
    image_requests: &[(String, toven_ports::ImageRequest)],
    image_phase: &dyn toven_ports::ImagePhase,
) -> AppResult<Vec<ProvenanceSubject>> {
    let mut subjects = Vec::new();
    for (_module, request) in image_requests {
        let Some(reference) = request.references().into_iter().next() else {
            continue;
        };
        if let Some(digest) = image_phase.resolve_digest(project_root, &reference)? {
            subjects.push(ProvenanceSubject::image(reference, digest));
        }
    }
    Ok(subjects)
}

/// Parse a `SHA256SUMS` body (`shasum -a 256` two-space format) into provenance
/// subjects, `sha256:`-prefixing each lowercase-hex digest. Each subject is
/// located for verification by its project-relative path — the entry's name
/// joined onto the manifest's own directory (`manifest_rel`), since a manifest
/// entry names a file sitting beside the manifest.
fn parse_manifest_subjects(manifest_rel: &str, path: &Path) -> AppResult<Vec<ProvenanceSubject>> {
    let text = read_string_bounded(path, MAX_MANIFEST_BYTES)?;
    let mut subjects = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let (hex, name) = line.split_once("  ").ok_or_else(|| {
            AppError::invalid_input(
                "release.provenance.manifest",
                format!("malformed manifest line '{line}' (expected '<hex>  <name>')"),
            )
        })?;
        validate_manifest_entry(line, hex, name)?;
        let subject_path = subject_file_path(manifest_rel, name);
        subjects.push(ProvenanceSubject::file(
            name,
            format!("sha256:{hex}"),
            subject_path,
        ));
    }
    Ok(subjects)
}

/// The project-relative path of a manifest entry: its `name` joined onto the
/// manifest's own directory. A manifest at `dist/SHA256SUMS` with entry
/// `toven.tar.gz` yields `dist/toven.tar.gz`; a manifest at the project root
/// yields the bare name. Forward slashes are used so the path is a stable,
/// platform-independent `gh attestation verify` argument.
fn subject_file_path(manifest_rel: &str, name: &str) -> String {
    Path::new(manifest_rel)
        .parent()
        .and_then(Path::to_str)
        .filter(|dir| !dir.is_empty())
        .map_or_else(|| name.to_string(), |dir| format!("{dir}/{name}"))
}

/// Reject a manifest entry whose digest is not a 64-char lowercase-hex sha256,
/// or whose name is not a safe bare file name sitting beside the manifest. The
/// name flows into the `gh attestation verify` argv as the subject's
/// project-relative path (joined onto the manifest directory) *and* is hashed
/// off disk, so a name that is empty, could be mistaken for a flag (a leading
/// `-`), or escapes the manifest directory (a path separator, `.`, or `..`)
/// would let an untrusted manifest read or verify a file outside the release
/// directory. It is validated at this trust boundary; Toven's own manifests
/// list bare file names.
fn validate_manifest_entry(line: &str, hex: &str, name: &str) -> AppResult<()> {
    let is_lower_hex = hex.len() == 64
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'));
    if !is_lower_hex {
        return Err(AppError::invalid_input(
            "release.provenance.manifest",
            format!(
                "malformed manifest line '{line}' (expected a 64-char lowercase-hex sha256 digest)"
            ),
        ));
    }
    if name.is_empty() || name.starts_with('-') {
        return Err(AppError::invalid_input(
            "release.provenance.manifest",
            format!(
                "malformed manifest line '{line}' (subject name must be non-empty and not begin \
                 with '-')"
            ),
        ));
    }
    if name.contains('/') || name.contains('\\') || name == "." || name == ".." {
        return Err(AppError::invalid_input(
            "release.provenance.manifest",
            format!(
                "unsafe manifest entry '{name}': a subject name must be a bare file name beside \
                 the manifest, with no path separators or '..'"
            ),
        ));
    }
    Ok(())
}

/// The final path component of a declared asset path.
fn asset_file_name(asset: &str) -> Option<&str> {
    Path::new(asset).file_name().and_then(|name| name.to_str())
}

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
fn attestation_not_found(outcome: &ToolOutcome) -> bool {
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
fn repo_view_argv() -> Vec<String> {
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
fn verify_argv(subject: &ProvenanceSubject, repo: &str) -> Vec<String> {
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
fn ensure_file_matches_digest(root: &Path, subject: &ProvenanceSubject) -> AppResult<()> {
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
    let mut file = std::fs::File::open(&path).map_err(|error| {
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::sync::Arc;

    use rskit_config::RawValue;
    use rskit_errors::ErrorCode;
    use rskit_fs::TempDir;
    use serde_json::json;
    use toven_model::{AbsPath, EcosystemId, Module, ModuleRef, RepoPath};
    use toven_ports::{
        CommonEcosystemConfig, DiscoverResponse, HostConfig, ImageConfig, Provider, ReleaseConfig,
        TaskIntent,
    };
    use toven_testkit::{
        FakeConfiguredAdapter, FakeImagePhase, FakeProvenancePhase, FakeProvider,
        FakeReleaseTarget, FakeToolRunner, RecordingReporter,
    };

    use super::{
        GhAttestationProvenance, ProvenanceOptions, ProvenancePhaseStatus, attestation_not_found,
        ensure_file_matches_digest, release_provenance, repo_view_argv, subject_file_path,
        verify_argv,
    };
    use rskit_version::semver::Version;
    use toven_core::config::{Document, ProjectConfig, TovenConfig};
    use toven_core::plan::PlanRequest;
    use toven_ports::{ProvenanceArtifact, ProvenancePhase, ProvenanceSubject};

    fn eid(id: &str) -> EcosystemId {
        EcosystemId::new(id).unwrap()
    }

    fn module(name: &str) -> Module {
        Module::new(
            ModuleRef::new(eid("rust"), name).unwrap(),
            RepoPath::new(format!("crates/{name}")).unwrap(),
        )
    }

    fn document() -> Document {
        let mut ecosystems = BTreeMap::new();
        ecosystems.insert(eid("rust"), RawValue::from(json!({ "release": {} })));
        Document {
            project: ProjectConfig {
                name: "demo".to_string(),
                root: ".".to_string(),
                base_ref: None,
            },
            toven: TovenConfig::default(),
            groups: BTreeMap::new(),
            overlays: Vec::new(),
            ecosystems,
            modules: BTreeMap::new(),
            members: Vec::new(),
            hooks: std::collections::BTreeMap::new(),
        }
    }

    fn request(root: &Path) -> PlanRequest {
        PlanRequest::new(
            "r1",
            "demo",
            TaskIntent::resolve("release"),
            AbsPath::new(root.to_str().unwrap()).unwrap(),
        )
    }

    fn provider_with_assets(assets: Vec<&str>) -> FakeProvider {
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![module("core")];
        let common = CommonEcosystemConfig {
            release: ReleaseConfig {
                host: Some(HostConfig {
                    forge: Some("github".to_string()),
                    assets: Some(assets.into_iter().map(str::to_string).collect()),
                    ..HostConfig::default()
                }),
                ..ReleaseConfig::default()
            },
            ..CommonEcosystemConfig::default()
        };
        let adapter = FakeConfiguredAdapter::new(eid("rust"))
            .with_response(response)
            .with_release_target(FakeReleaseTarget::new())
            .with_common(common);
        FakeProvider::new(eid("rust")).with_adapter(adapter)
    }

    fn provider_with_image(assets: Vec<&str>) -> FakeProvider {
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![module("core")];
        let image = ImageConfig {
            registry: "ghcr.io/acme".into(),
            mirrors: vec![],
            name: "toven".into(),
            tag: Some("{version}".into()),
            context: Some("services/api".into()),
            dockerfile: None,
            sign: true,
        };
        let host = if assets.is_empty() {
            None
        } else {
            Some(HostConfig {
                forge: Some("github".to_string()),
                assets: Some(assets.into_iter().map(str::to_string).collect()),
                ..HostConfig::default()
            })
        };
        let common = CommonEcosystemConfig {
            release: ReleaseConfig {
                image: Some(image),
                host,
                ..ReleaseConfig::default()
            },
            ..CommonEcosystemConfig::default()
        };
        let adapter = FakeConfiguredAdapter::new(eid("rust"))
            .with_response(response)
            .with_release_target(
                FakeReleaseTarget::new().with_declared_version(Version::new(1, 0, 0)),
            )
            .with_common(common);
        FakeProvider::new(eid("rust")).with_adapter(adapter)
    }

    fn write_manifest(root: &Path, lines: &[(&str, &str)]) {
        let dist = root.join("dist");
        std::fs::create_dir_all(&dist).unwrap();
        let mut body = String::new();
        for (hex, name) in lines {
            body.push_str(hex);
            body.push_str("  ");
            body.push_str(name);
            body.push('\n');
        }
        std::fs::write(dist.join("SHA256SUMS"), body).unwrap();
    }

    #[test]
    fn verifies_exactly_the_published_manifest_subjects() {
        let root = TempDir::new().unwrap();
        write_manifest(
            root.path(),
            &[
                ("a".repeat(64).as_str(), "toven.tar.gz"),
                ("b".repeat(64).as_str(), "toven-sbom.cdx.json"),
            ],
        );
        let provider = provider_with_assets(vec![
            "dist/toven.tar.gz",
            "dist/SHA256SUMS",
            "dist/toven-sbom.cdx.json",
        ]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let phase = FakeProvenancePhase::new();
        let mut reporter = RecordingReporter::new();

        let report = release_provenance(
            &request(root.path()),
            &document(),
            &providers,
            &phase,
            &FakeImagePhase::new(),
            ProvenanceOptions::default(),
            &mut reporter,
        )
        .expect("provenance runs");

        assert!(!report.preview);
        assert_eq!(report.status, ProvenancePhaseStatus::Verified);
        // Subjects are exactly the manifest entries, sha256:-prefixed, each
        // located by its project-relative path beside the manifest.
        let names: Vec<&str> = report
            .subjects
            .iter()
            .map(|s| s.subject.name.as_str())
            .collect();
        assert_eq!(names, vec!["toven.tar.gz", "toven-sbom.cdx.json"]);
        assert!(
            report
                .subjects
                .iter()
                .all(|s| s.subject.digest.starts_with("sha256:"))
        );
        assert!(
            report
                .subjects
                .iter()
                .all(|s| s.status == ProvenancePhaseStatus::Verified)
        );
        assert_eq!(
            report.subjects[0].subject.artifact,
            ProvenanceArtifact::File("dist/toven.tar.gz".to_string())
        );
        // The adapter was handed exactly those subjects, once.
        let calls = phase.calls();
        assert_eq!(calls.len(), 1);
        let handed: Vec<ProvenanceSubject> =
            report.subjects.iter().map(|s| s.subject.clone()).collect();
        assert_eq!(calls[0].subjects, handed);
    }

    #[test]
    fn dry_run_reports_missing_without_failing() {
        let root = TempDir::new().unwrap();
        write_manifest(root.path(), &[("c".repeat(64).as_str(), "toven.tar.gz")]);
        let provider = provider_with_assets(vec!["dist/toven.tar.gz", "dist/SHA256SUMS"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let phase = FakeProvenancePhase::new().with_existing(false);
        let mut reporter = RecordingReporter::new();

        let report = release_provenance(
            &request(root.path()),
            &document(),
            &providers,
            &phase,
            &FakeImagePhase::new(),
            ProvenanceOptions { dry_run: true },
            &mut reporter,
        )
        .expect("preview runs");

        assert!(report.preview);
        assert_eq!(report.status, ProvenancePhaseStatus::Missing);
        assert!(phase.calls().is_empty(), "preview must not enforce");
    }

    #[test]
    fn dry_run_reports_present_attestations() {
        let root = TempDir::new().unwrap();
        write_manifest(root.path(), &[("d".repeat(64).as_str(), "toven.tar.gz")]);
        let provider = provider_with_assets(vec!["dist/toven.tar.gz", "dist/SHA256SUMS"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let phase = FakeProvenancePhase::new().with_existing(true);
        let mut reporter = RecordingReporter::new();

        let report = release_provenance(
            &request(root.path()),
            &document(),
            &providers,
            &phase,
            &FakeImagePhase::new(),
            ProvenanceOptions { dry_run: true },
            &mut reporter,
        )
        .expect("preview runs");

        assert_eq!(report.status, ProvenancePhaseStatus::Present);
        assert!(phase.calls().is_empty());
    }

    #[test]
    fn fails_closed_when_a_subject_lacks_an_attestation() {
        let root = TempDir::new().unwrap();
        write_manifest(root.path(), &[("e".repeat(64).as_str(), "toven.tar.gz")]);
        let provider = provider_with_assets(vec!["dist/toven.tar.gz", "dist/SHA256SUMS"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let phase = FakeProvenancePhase::new().with_existing(false);
        let mut reporter = RecordingReporter::new();

        let error = release_provenance(
            &request(root.path()),
            &document(),
            &providers,
            &phase,
            &FakeImagePhase::new(),
            ProvenanceOptions::default(),
            &mut reporter,
        )
        .expect_err("a missing attestation must fail closed");
        assert!(
            error
                .to_string()
                .contains("no build-provenance attestation"),
            "{error}"
        );
    }

    #[test]
    fn fails_closed_when_no_manifest_is_declared() {
        let root = TempDir::new().unwrap();
        let provider = provider_with_assets(vec!["dist/toven.tar.gz"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let phase = FakeProvenancePhase::new();
        let mut reporter = RecordingReporter::new();

        let error = release_provenance(
            &request(root.path()),
            &document(),
            &providers,
            &phase,
            &FakeImagePhase::new(),
            ProvenanceOptions::default(),
            &mut reporter,
        )
        .expect_err("no manifest must fail closed");
        assert!(error.to_string().contains("SHA256SUMS"), "{error}");
    }

    #[test]
    fn verifies_manifest_subjects_and_pushed_image_digests() {
        let root = TempDir::new().unwrap();
        write_manifest(root.path(), &[("a".repeat(64).as_str(), "toven.tar.gz")]);
        let provider = provider_with_image(vec!["dist/toven.tar.gz", "dist/SHA256SUMS"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let phase = FakeProvenancePhase::new();
        let image = FakeImagePhase::new().with_existing_digest("sha256:img");
        let mut reporter = RecordingReporter::new();

        let report = release_provenance(
            &request(root.path()),
            &document(),
            &providers,
            &phase,
            &image,
            ProvenanceOptions::default(),
            &mut reporter,
        )
        .expect("provenance runs");

        let names: Vec<&str> = report
            .subjects
            .iter()
            .map(|s| s.subject.name.as_str())
            .collect();
        assert!(names.contains(&"toven.tar.gz"), "{names:?}");
        assert!(
            names.contains(&"ghcr.io/acme/toven:1.0.0"),
            "the pushed image digest is attested: {names:?}"
        );
        let image_subject = report
            .subjects
            .iter()
            .find(|s| s.subject.name == "ghcr.io/acme/toven:1.0.0")
            .expect("image subject present");
        assert_eq!(image_subject.subject.digest, "sha256:img");
        assert_eq!(
            image_subject.subject.artifact,
            ProvenanceArtifact::Image("ghcr.io/acme/toven:1.0.0".to_string())
        );
    }

    #[test]
    fn verifies_an_image_only_release_without_a_manifest() {
        let root = TempDir::new().unwrap();
        let provider = provider_with_image(vec![]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let phase = FakeProvenancePhase::new();
        let image = FakeImagePhase::new().with_existing_digest("sha256:img");
        let mut reporter = RecordingReporter::new();

        let report = release_provenance(
            &request(root.path()),
            &document(),
            &providers,
            &phase,
            &image,
            ProvenanceOptions::default(),
            &mut reporter,
        )
        .expect("image-only provenance runs");

        assert_eq!(report.subjects.len(), 1);
        assert_eq!(report.subjects[0].subject.name, "ghcr.io/acme/toven:1.0.0");
    }

    #[test]
    fn image_provenance_requires_the_primary_registry_digest() {
        let root = TempDir::new().unwrap();
        let provider = provider_with_image(vec![]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let phase = FakeProvenancePhase::new();
        let image =
            FakeImagePhase::new().with_reference_digest("docker.io/acme/toven:1.0.0", "sha256:img");
        let mut reporter = RecordingReporter::new();

        let error = release_provenance(
            &request(root.path()),
            &document(),
            &providers,
            &phase,
            &image,
            ProvenanceOptions::default(),
            &mut reporter,
        )
        .expect_err("a mirror digest must not substitute for the primary");

        assert!(error.to_string().contains("release image"), "{error}");
        assert!(phase.calls().is_empty());
    }

    #[test]
    fn fails_closed_when_an_image_was_not_pushed_and_no_manifest_exists() {
        let root = TempDir::new().unwrap();
        let provider = provider_with_image(vec![]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let phase = FakeProvenancePhase::new();
        // A phase whose references resolve no digest: the image was never pushed.
        let image = FakeImagePhase::new();
        let mut reporter = RecordingReporter::new();

        let error = release_provenance(
            &request(root.path()),
            &document(),
            &providers,
            &phase,
            &image,
            ProvenanceOptions::default(),
            &mut reporter,
        )
        .expect_err("an unpublished image must fail closed");
        assert!(error.to_string().contains("release image"), "{error}");
    }

    #[test]
    fn surfaces_an_attestation_failure() {
        let root = TempDir::new().unwrap();
        write_manifest(root.path(), &[("a".repeat(64).as_str(), "toven.tar.gz")]);
        let provider = provider_with_assets(vec!["dist/toven.tar.gz", "dist/SHA256SUMS"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let phase = FakeProvenancePhase::failing("gh attestation missing");
        let image = FakeImagePhase::new();
        let mut reporter = RecordingReporter::new();

        let error = release_provenance(
            &request(root.path()),
            &document(),
            &providers,
            &phase,
            &image,
            ProvenanceOptions::default(),
            &mut reporter,
        )
        .expect_err("an attestation failure must surface");
        assert!(
            error.to_string().contains("gh attestation missing"),
            "{error}"
        );
    }

    #[test]
    fn rejects_a_malformed_manifest_digest() {
        let root = TempDir::new().unwrap();
        write_manifest(root.path(), &[("nothex", "toven.tar.gz")]);
        let provider = provider_with_assets(vec!["dist/toven.tar.gz", "dist/SHA256SUMS"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let phase = FakeProvenancePhase::new();
        let image = FakeImagePhase::new();
        let mut reporter = RecordingReporter::new();

        let error = release_provenance(
            &request(root.path()),
            &document(),
            &providers,
            &phase,
            &image,
            ProvenanceOptions::default(),
            &mut reporter,
        )
        .expect_err("a malformed digest must be rejected");
        assert!(error.to_string().contains("lowercase-hex"), "{error}");
    }

    #[test]
    fn verify_argv_targets_a_file_subject_by_path_and_repo() {
        let subject = ProvenanceSubject::file("toven.tar.gz", "sha256:abc", "dist/toven.tar.gz");
        let argv = verify_argv(&subject, "acme/toven");
        assert_eq!(argv[0], "attestation");
        assert_eq!(argv[1], "verify");
        assert_eq!(argv[2], "dist/toven.tar.gz");
        assert!(
            argv.windows(2).any(|pair| pair == ["--repo", "acme/toven"]),
            "{argv:?}"
        );
        // Verification is bound to the trusted builder, not just the repo, via
        // the repository-qualified workflow path gh matches on.
        assert!(
            argv.windows(2).any(|pair| pair
                == [
                    "--signer-workflow",
                    "acme/toven/.github/workflows/release.yml"
                ]),
            "{argv:?}"
        );
        // The digest never reaches the argv: gh recomputes it from the file
        // (Toven pre-checks the file bytes against the manifest digest itself).
        assert!(!argv.iter().any(|token| token == "sha256:abc"));
    }

    #[test]
    fn verify_argv_pins_an_image_subject_to_its_digest() {
        let subject = ProvenanceSubject::image("ghcr.io/acme/toven:1.0.0", "sha256:img");
        let argv = verify_argv(&subject, "acme/toven");
        // The image is pinned by digest so the registry cannot resolve the tag
        // to a different digest than the one Toven collected.
        assert_eq!(argv[2], "oci://ghcr.io/acme/toven:1.0.0@sha256:img");
        assert!(
            argv.windows(2).any(|pair| pair
                == [
                    "--signer-workflow",
                    "acme/toven/.github/workflows/release.yml"
                ]),
            "{argv:?}"
        );
    }

    #[test]
    fn repo_view_argv_is_a_read_only_slug_probe() {
        let argv = repo_view_argv();
        assert_eq!(argv[0], "repo");
        assert_eq!(argv[1], "view");
        assert!(argv.iter().any(|token| token == "nameWithOwner"));
    }

    #[test]
    fn subject_file_path_joins_the_manifest_directory() {
        assert_eq!(
            subject_file_path("dist/SHA256SUMS", "toven.tar.gz"),
            "dist/toven.tar.gz"
        );
        assert_eq!(
            subject_file_path("SHA256SUMS", "toven.tar.gz"),
            "toven.tar.gz"
        );
    }

    #[test]
    fn dry_run_reports_each_subject_present_or_missing_independently() {
        let root = TempDir::new().unwrap();
        write_manifest(
            root.path(),
            &[
                ("a".repeat(64).as_str(), "toven.tar.gz"),
                ("b".repeat(64).as_str(), "toven-sbom.cdx.json"),
            ],
        );
        let provider = provider_with_assets(vec![
            "dist/toven.tar.gz",
            "dist/SHA256SUMS",
            "dist/toven-sbom.cdx.json",
        ]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        // The archive is attested; the SBOM is not.
        let phase = FakeProvenancePhase::new().with_missing("toven-sbom.cdx.json");
        let mut reporter = RecordingReporter::new();

        let report = release_provenance(
            &request(root.path()),
            &document(),
            &providers,
            &phase,
            &FakeImagePhase::new(),
            ProvenanceOptions { dry_run: true },
            &mut reporter,
        )
        .expect("preview runs");

        // The aggregate is Missing, but each subject keeps its own result: the
        // attested archive is not masked by the unattested SBOM.
        assert_eq!(report.status, ProvenancePhaseStatus::Missing);
        let archive = report
            .subjects
            .iter()
            .find(|s| s.subject.name == "toven.tar.gz")
            .expect("archive subject");
        let sbom = report
            .subjects
            .iter()
            .find(|s| s.subject.name == "toven-sbom.cdx.json")
            .expect("sbom subject");
        assert_eq!(archive.status, ProvenancePhaseStatus::Present);
        assert_eq!(sbom.status, ProvenancePhaseStatus::Missing);
    }

    #[test]
    fn rejects_a_traversing_manifest_entry() {
        let root = TempDir::new().unwrap();
        write_manifest(root.path(), &[("a".repeat(64).as_str(), "../secret")]);
        let provider = provider_with_assets(vec!["dist/toven.tar.gz", "dist/SHA256SUMS"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let phase = FakeProvenancePhase::new();
        let mut reporter = RecordingReporter::new();

        let error = release_provenance(
            &request(root.path()),
            &document(),
            &providers,
            &phase,
            &FakeImagePhase::new(),
            ProvenanceOptions::default(),
            &mut reporter,
        )
        .expect_err("a traversing manifest entry must be rejected");
        assert!(error.to_string().contains("bare file name"), "{error}");
    }

    #[test]
    fn file_digest_check_accepts_a_matching_file_and_rejects_a_mismatch() {
        let root = TempDir::new().unwrap();
        let dist = root.path().join("dist");
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::write(dist.join("toven.tar.gz"), b"").unwrap();
        // The sha256 of an empty file is a well-known vector.
        let empty = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

        let matching = ProvenanceSubject::file(
            "toven.tar.gz",
            format!("sha256:{empty}"),
            "dist/toven.tar.gz",
        );
        ensure_file_matches_digest(root.path(), &matching).expect("matching digest passes");

        let mismatch = ProvenanceSubject::file(
            "toven.tar.gz",
            "sha256:".to_string() + &"0".repeat(64),
            "dist/toven.tar.gz",
        );
        let error =
            ensure_file_matches_digest(root.path(), &mismatch).expect_err("mismatch fails closed");
        assert!(error.to_string().contains("does not match"), "{error}");
    }

    #[test]
    fn image_subject_skips_the_file_digest_check() {
        let root = TempDir::new().unwrap();
        let subject = ProvenanceSubject::image("ghcr.io/acme/toven:1.0.0", "sha256:img");
        // No file exists, but an image subject carries its digest in the pinned
        // reference, so the on-disk check is a no-op.
        ensure_file_matches_digest(root.path(), &subject).expect("image subject is a no-op");
    }

    fn failed_gh(stderr: &str) -> super::ToolOutcome {
        super::ToolOutcome::new(Some(1), String::new(), stderr.to_string())
    }

    #[test]
    fn classifies_the_no_attestations_message_as_absent() {
        assert!(attestation_not_found(&failed_gh(
            "Error: no attestations found for subject"
        )));
    }

    #[test]
    fn classifies_an_attestations_endpoint_404_as_absent() {
        // A current gh (>= 2.67.0) surfaces an unattested digest as an HTTP 404
        // on the attestations lookup endpoint and exits non-zero.
        let stderr = "Error: HTTP 404: Not Found (https://api.github.com/repos/kbukum/toven/\
                      attestations/sha256:02d56dac?per_page=30&predicate_type=https%3A%2F%2F\
                      slsa.dev%2Fprovenance%2Fv1)";
        assert!(attestation_not_found(&failed_gh(stderr)));
    }

    #[test]
    fn fails_closed_on_a_non_attestation_404() {
        // A 404 that never reached the attestations lookup (e.g. an inaccessible
        // repository) is not an absence signal and must fail closed.
        let stderr = "Error: HTTP 404: Not Found (https://api.github.com/repos/kbukum/toven)";
        assert!(!attestation_not_found(&failed_gh(stderr)));
    }

    #[test]
    fn fails_closed_on_an_auth_error() {
        assert!(!attestation_not_found(&failed_gh(
            "Error: HTTP 401: Bad credentials"
        )));
    }

    #[test]
    fn gh_attestation_verify_builds_repo_and_subject_argv_first() {
        let runner = FakeToolRunner::new().with_stdout("acme/toven\n");
        let phase = GhAttestationProvenance::new(Arc::new(runner.clone()));
        let subject = ProvenanceSubject::image("ghcr.io/acme/toven:1.0.0", "sha256:img");

        phase
            .verify(Path::new("/repo"), std::slice::from_ref(&subject))
            .expect("provenance verifies");

        let requests = runner.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[0].argv,
            vec![
                "gh",
                "repo",
                "view",
                "--json",
                "nameWithOwner",
                "--jq",
                ".nameWithOwner",
            ]
        );
        assert_eq!(
            requests[1].argv,
            vec![
                "gh",
                "attestation",
                "verify",
                "oci://ghcr.io/acme/toven:1.0.0@sha256:img",
                "--repo",
                "acme/toven",
                "--signer-workflow",
                "acme/toven/.github/workflows/release.yml",
            ]
        );
        assert!(
            requests
                .iter()
                .all(|request| request.forward_env.is_empty())
        );
        assert!(
            requests
                .iter()
                .flat_map(|request| &request.argv)
                .all(|arg| !arg.contains("ghp_secret")),
            "argv leaked a token value: {requests:?}"
        );
    }

    #[test]
    fn gh_attestation_absence_is_not_a_tool_failure() {
        let runner = FakeToolRunner::new().with_stdout("acme/toven\n");
        let phase = GhAttestationProvenance::new(Arc::new(runner.clone()));
        let subject = ProvenanceSubject::image("ghcr.io/acme/toven:1.0.0", "sha256:img");
        assert!(
            phase
                .attestation_exists(Path::new("/repo"), &subject)
                .expect("initial attestation check succeeds")
        );
        let runner_state = runner.clone();
        let _ = runner_state
            .with_exit_code(Some(1))
            .with_stderr("no attestations found for subject");

        let missing = phase
            .attestation_exists(Path::new("/repo"), &subject)
            .expect("missing attestation is absence");

        assert!(!missing);
        let requests = runner.requests();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[2].argv[0], "gh");
        assert_eq!(&requests[2].argv[1..3], &["attestation", "verify"]);
    }

    #[test]
    fn gh_attestation_real_failure_maps_to_process_failure() {
        let runner = FakeToolRunner::new().with_stdout("acme/toven\n");
        let phase = GhAttestationProvenance::new(Arc::new(runner.clone()));
        let subject = ProvenanceSubject::image("ghcr.io/acme/toven:1.0.0", "sha256:img");
        assert!(
            phase
                .attestation_exists(Path::new("/repo"), &subject)
                .expect("initial attestation check succeeds")
        );
        let _ = runner
            .with_exit_code(Some(1))
            .with_stderr("HTTP 401: Bad credentials");

        let error = phase
            .attestation_exists(Path::new("/repo"), &subject)
            .expect_err("auth failure fails closed");

        assert_eq!(error.code(), ErrorCode::Internal);
        assert!(
            error.to_string().contains("gh exited with code 1"),
            "{error}"
        );
        assert!(
            error
                .to_string()
                .contains("refusing to treat the result as absent"),
            "{error}"
        );
    }

    #[test]
    fn gh_attestation_repo_spawn_failure_surfaces() {
        let runner = FakeToolRunner::new().with_spawn_failure("gh not found");
        let phase = GhAttestationProvenance::new(Arc::new(runner));
        let subject = ProvenanceSubject::image("ghcr.io/acme/toven:1.0.0", "sha256:img");

        let error = phase
            .verify(Path::new("/repo"), &[subject])
            .expect_err("spawn failure surfaces");

        assert_eq!(error.code(), ErrorCode::Internal);
        assert!(error.to_string().contains("gh not found"), "{error}");
    }
}
