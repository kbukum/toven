//! `release provenance` verb and the `gh attestation` [`ProvenancePhase`]
//! adapter.
//!
//! The engine owns provenance *policy*: which subjects an attestation is cut
//! over. Those subjects are exactly what was actually published — the entries
//! of the declared `SHA256SUMS` manifest (each archive/SBOM and its digest) and
//! the live digest of every pushed image reference — so the attestation covers
//! the released bytes and nothing else. The only reusable primitive is "run a
//! subprocess" ([`rskit_process`]); [`GhAttestationProvenance`] shells to
//! `gh attestation` argv-only, reading the ambient forge token from the
//! environment — it embeds no secret and captures none.
//!
//! Provenance is immutable: an attestation is cut once over the published
//! subjects. The mutation-free `--dry-run` preview only queries whether an
//! attestation already exists and never attests.

use std::collections::BTreeMap;
use std::path::Path;

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::safe_join;
use rskit_fs::sync_io::file::read_string_bounded;
use rskit_process::{CapturedIo, OutputPolicy, ProcessConfig, ProcessIo, ProcessSpec, run};
use toven_ports::{ProvenanceOutcome, ProvenancePhase, ProvenanceSubject, Provider, Reporter};

use super::plan::{release_targets, resolve_release_settings};
use crate::config::Document;
use crate::federation::resolve::PathDriverLocator;
use crate::plan::{PlanRequest, prepare_front};

/// The canonical file name of the checksum manifest whose entries are the
/// provenance subjects. A declared asset whose file name is exactly this is the
/// manifest; one beginning `SHA256SUMS.` is a signature sidecar, not a subject.
const MANIFEST_NAME: &str = "SHA256SUMS";

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
    /// An attestation was cut over the published subjects.
    Attested,
    /// Every subject already carried a matching attestation (idempotent re-run).
    AlreadyComplete,
    /// `--dry-run`: no attestation exists yet, so one would be cut.
    WouldAttest,
    /// `--dry-run`: every subject already carries an attestation.
    AlreadyPresent,
}

impl ProvenancePhaseStatus {
    /// Canonical wire/report name for the status.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Attested => "attested",
            Self::AlreadyComplete => "already-complete",
            Self::WouldAttest => "would-attest",
            Self::AlreadyPresent => "already-present",
        }
    }
}

/// A read-only projection of the provenance phase over the release scope.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProvenanceReport {
    /// Whether this report is a mutation-free `--dry-run` preview.
    pub preview: bool,
    /// The subjects the attestation covers, in manifest order.
    pub subjects: Vec<ProvenanceSubject>,
    /// The resolved status of the phase.
    pub status: ProvenancePhaseStatus,
}

/// Options controlling the provenance phase.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProvenanceOptions {
    /// Preview the phase mutation-free: query whether an attestation already
    /// exists but never attest.
    pub dry_run: bool,
}

/// Attest SLSA provenance over exactly the published subjects — the entries of
/// the declared `SHA256SUMS` manifest plus the live digest of every pushed
/// image reference.
///
/// With `options.dry_run`, the phase is a mutation-free preview: it queries
/// whether an attestation already exists for the subjects but never attests.
/// Otherwise it hands the adapter exactly the published subjects to attest.
///
/// # Errors
/// Fails closed with a typed error when neither a `SHA256SUMS` manifest nor a
/// published image is available to attest, the manifest file is missing or
/// malformed, or nothing resolves to a subject — and propagates
/// configuration/discovery/graph failures and attestation-tool failures.
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

    let status = if options.dry_run {
        let mut all_present = true;
        for subject in &subjects {
            if !provenance_phase.attestation_exists(project_root, subject)? {
                all_present = false;
            }
        }
        if all_present {
            ProvenancePhaseStatus::AlreadyPresent
        } else {
            ProvenancePhaseStatus::WouldAttest
        }
    } else {
        match provenance_phase.attest(project_root, &subjects)? {
            ProvenanceOutcome::AlreadyComplete => ProvenancePhaseStatus::AlreadyComplete,
            _ => ProvenancePhaseStatus::Attested,
        }
    };

    Ok(ProvenanceReport {
        preview: options.dry_run,
        subjects,
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
    settings: &BTreeMap<toven_model::ModuleKey, super::ResolvedReleaseSettings>,
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
        let manifest_subjects = parse_manifest_subjects(&path)?;
        if manifest_subjects.is_empty() {
            return Err(AppError::invalid_input(
                "release.host.assets",
                format!("manifest '{manifest}' lists no subjects to attest"),
            ));
        }
        subjects.extend(manifest_subjects);
    } else if image_requests.is_empty() {
        return Err(AppError::invalid_input(
            "release.host.assets",
            format!(
                "no '{MANIFEST_NAME}' manifest and no image are declared; provenance attests \
                 exactly the published subjects (manifest entries and pushed image digests)"
            ),
        ));
    }

    subjects.extend(image_subjects(project_root, image_requests, image_phase)?);

    if subjects.is_empty() {
        return Err(AppError::invalid_input(
            "release.provenance.subjects",
            "nothing published to attest: the declared image references resolve to no pushed \
             digest — run `toven release image` first",
        ));
    }
    Ok(subjects)
}

/// The live digest of each published image reference, as a provenance subject.
/// A reference the registry does not yet resolve (the image was not pushed)
/// contributes nothing — provenance attests only what was actually published.
/// The primary reference is preferred; mirrors carry the same digest, so each
/// image yields at most one subject.
fn image_subjects(
    project_root: &Path,
    image_requests: &[(String, toven_ports::ImageRequest)],
    image_phase: &dyn toven_ports::ImagePhase,
) -> AppResult<Vec<ProvenanceSubject>> {
    let mut subjects = Vec::new();
    for (_module, request) in image_requests {
        for reference in request.references() {
            if let Some(digest) = image_phase.resolve_digest(project_root, &reference)? {
                subjects.push(ProvenanceSubject::new(reference, digest));
                break;
            }
        }
    }
    Ok(subjects)
}

/// Parse a `SHA256SUMS` body (`shasum -a 256` two-space format) into
/// provenance subjects, `sha256:`-prefixing each lowercase-hex digest.
fn parse_manifest_subjects(path: &Path) -> AppResult<Vec<ProvenanceSubject>> {
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
        subjects.push(ProvenanceSubject::new(name, format!("sha256:{hex}")));
    }
    Ok(subjects)
}

/// Reject a manifest entry whose digest is not a 64-char lowercase-hex sha256,
/// or whose name is empty or could be mistaken for a flag on the attestation
/// argv (a leading `-`). The digest and name flow into `gh attestation`
/// arguments, so they are validated at this trust boundary.
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
    Ok(())
}

/// The final path component of a declared asset path.
fn asset_file_name(asset: &str) -> Option<&str> {
    Path::new(asset).file_name().and_then(|name| name.to_str())
}

/// A `gh attestation`-backed [`ProvenancePhase`].
///
/// Construction is stateless. `gh attestation` is invoked argv-only through
/// [`rskit_process`]; the ambient forge token the runner provides is inherited
/// from the environment, and no secret is placed on argv or captured.
#[derive(Debug, Clone)]
pub struct GhAttestationProvenance {
    timeout: std::time::Duration,
}

impl GhAttestationProvenance {
    /// Construct a `gh attestation` provenance phase with the default timeout.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            timeout: PROVENANCE_TIMEOUT,
        }
    }

    /// Run an argv-only `gh` invocation rooted at `root`, returning captured
    /// stdout and whether the process exited zero.
    fn run(&self, root: &Path, argv: Vec<String>) -> AppResult<(bool, String)> {
        let spec = ProcessSpec::new("gh").args(argv).dir(root);
        let config = ProcessConfig::default()
            .with_timeout(Some(self.timeout))
            .with_io(ProcessIo::captured(CapturedIo::new().with_output(
                OutputPolicy::captured().with_max_output_bytes(MAX_PROVENANCE_OUTPUT_BYTES),
            )));
        let result = run(&spec, &config)?;
        Ok((result.success(), result.stdout))
    }
}

impl Default for GhAttestationProvenance {
    fn default() -> Self {
        Self::new()
    }
}

impl ProvenancePhase for GhAttestationProvenance {
    fn attest(&self, root: &Path, subjects: &[ProvenanceSubject]) -> AppResult<ProvenanceOutcome> {
        if subjects.is_empty() {
            return Err(AppError::invalid_input(
                "release.provenance.subjects",
                "no subjects to attest",
            ));
        }
        let mut all_present = true;
        for subject in subjects {
            if !self.attestation_exists(root, subject)? {
                all_present = false;
            }
        }
        if all_present {
            return Ok(ProvenanceOutcome::AlreadyComplete);
        }
        for subject in subjects {
            let (ok, _) = self.run(root, attest_argv(subject))?;
            if !ok {
                return Err(AppError::new(
                    ErrorCode::Internal,
                    format!(
                        "attestation for subject '{}' failed; the release is not attested",
                        subject.name
                    ),
                ));
            }
        }
        Ok(ProvenanceOutcome::Attested)
    }

    fn attestation_exists(&self, root: &Path, subject: &ProvenanceSubject) -> AppResult<bool> {
        let (ok, _) = self.run(root, verify_argv(subject))?;
        Ok(ok)
    }
}

/// Build the argv-only `gh attestation` invocation that cuts an attestation
/// over `subject`'s digest.
fn attest_argv(subject: &ProvenanceSubject) -> Vec<String> {
    vec![
        "attestation".to_string(),
        "sign".to_string(),
        "--digest".to_string(),
        subject.digest.clone(),
        "--name".to_string(),
        subject.name.clone(),
    ]
}

/// Build the argv-only `gh attestation verify` invocation that queries whether
/// an attestation already exists for `subject`.
fn verify_argv(subject: &ProvenanceSubject) -> Vec<String> {
    vec![
        "attestation".to_string(),
        "verify".to_string(),
        "--digest".to_string(),
        subject.digest.clone(),
    ]
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use rskit_config::RawValue;
    use rskit_fs::TempDir;
    use serde_json::json;
    use toven_model::{AbsPath, EcosystemId, Module, ModuleRef, RepoPath};
    use toven_ports::{
        CommonEcosystemConfig, DiscoverResponse, HostConfig, ImageConfig, ProvenanceOutcome,
        Provider, ReleaseConfig, TaskIntent,
    };
    use toven_testkit::{
        FakeConfiguredAdapter, FakeImagePhase, FakeProvenancePhase, FakeProvider,
        FakeReleaseTarget, RecordingReporter,
    };

    use super::{
        ProvenanceOptions, ProvenancePhaseStatus, attest_argv, release_provenance, verify_argv,
    };
    use crate::config::{Document, ProjectConfig, TovenConfig};
    use crate::plan::PlanRequest;
    use rskit_version::semver::Version;
    use toven_ports::ProvenanceSubject;

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
    fn attests_exactly_the_published_manifest_subjects() {
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
        assert_eq!(report.status, ProvenancePhaseStatus::Attested);
        // Subjects are exactly the manifest entries, sha256:-prefixed.
        let names: Vec<&str> = report.subjects.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["toven.tar.gz", "toven-sbom.cdx.json"]);
        assert!(
            report
                .subjects
                .iter()
                .all(|s| s.digest.starts_with("sha256:"))
        );
        // The adapter was handed exactly those subjects, once.
        let calls = phase.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].subjects, report.subjects);
    }

    #[test]
    fn dry_run_previews_without_attesting() {
        let root = TempDir::new().unwrap();
        write_manifest(root.path(), &[("c".repeat(64).as_str(), "toven.tar.gz")]);
        let provider = provider_with_assets(vec!["dist/toven.tar.gz", "dist/SHA256SUMS"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let phase = FakeProvenancePhase::new();
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
        assert_eq!(report.status, ProvenancePhaseStatus::WouldAttest);
        assert!(phase.calls().is_empty(), "preview must not attest");
    }

    #[test]
    fn dry_run_reports_an_existing_attestation() {
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

        assert_eq!(report.status, ProvenancePhaseStatus::AlreadyPresent);
        assert!(phase.calls().is_empty());
    }

    #[test]
    fn maps_already_complete_outcome() {
        let root = TempDir::new().unwrap();
        write_manifest(root.path(), &[("e".repeat(64).as_str(), "toven.tar.gz")]);
        let provider = provider_with_assets(vec!["dist/toven.tar.gz", "dist/SHA256SUMS"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let phase = FakeProvenancePhase::new().with_outcome(ProvenanceOutcome::AlreadyComplete);
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
        .expect("runs");
        assert_eq!(report.status, ProvenancePhaseStatus::AlreadyComplete);
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
    fn attests_manifest_subjects_and_pushed_image_digests() {
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

        let names: Vec<&str> = report.subjects.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"toven.tar.gz"), "{names:?}");
        assert!(
            names.contains(&"ghcr.io/acme/toven:1.0.0"),
            "the pushed image digest is attested: {names:?}"
        );
        let image_subject = report
            .subjects
            .iter()
            .find(|s| s.name == "ghcr.io/acme/toven:1.0.0")
            .expect("image subject present");
        assert_eq!(image_subject.digest, "sha256:img");
    }

    #[test]
    fn attests_an_image_only_release_without_a_manifest() {
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
        assert_eq!(report.subjects[0].name, "ghcr.io/acme/toven:1.0.0");
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
    fn attest_and_verify_argv_carry_the_digest() {
        let subject = ProvenanceSubject::new("toven.tar.gz", "sha256:abc");
        let attest = attest_argv(&subject);
        assert_eq!(attest[0], "attestation");
        assert!(attest.iter().any(|token| token == "sha256:abc"));
        let verify = verify_argv(&subject);
        assert!(verify.iter().any(|token| token == "verify"));
        assert!(verify.iter().any(|token| token == "sha256:abc"));
    }
}
