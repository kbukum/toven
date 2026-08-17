use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use rskit_config::RawValue;
use rskit_errors::ErrorCode;
use rskit_fs::TempDir;
use serde_json::json;
use toven_model::{AbsPath, EcosystemId, Module, ModuleRef, RepoPath};
use toven_ports::{
    BaselineSpec, CommonEcosystemConfig, DiscoverResponse, HostConfig, ImageConfig, Provider,
    ReleaseConfig, TaskIntent,
};
use toven_testkit::{
    FakeConfiguredAdapter, FakeImagePhase, FakeProvenancePhase, FakeProvider, FakeReleaseTarget,
    FakeToolRunner, FakeVcsReader, RecordingReporter,
};

use super::attestation::{
    attestation_not_found, ensure_file_matches_digest, repo_view_argv, verify_argv,
};
use super::phase::subject_file_path;
use super::{
    GhAttestationProvenance, ProvenanceOptions, ProvenanceOutcome, ProvenancePhaseStatus,
    provenance_operation, release_provenance,
};
use rskit_version::semver::Version;
use toven_core::config::{Document, ProjectConfig, TovenConfig};
use toven_core::federation::MemberVcsReaders;
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
        units: std::collections::BTreeMap::new(),
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
        .with_release_target(FakeReleaseTarget::new().with_declared_version(Version::new(1, 0, 0)))
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
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let phase = FakeProvenancePhase::new();
    let mut reporter = RecordingReporter::new();

    let report = release_provenance(
        &request(root.path()),
        &document(),
        &providers,
        &readers,
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
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let phase = FakeProvenancePhase::new().with_existing(false);
    let mut reporter = RecordingReporter::new();

    let report = release_provenance(
        &request(root.path()),
        &document(),
        &providers,
        &readers,
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
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let phase = FakeProvenancePhase::new().with_existing(true);
    let mut reporter = RecordingReporter::new();

    let report = release_provenance(
        &request(root.path()),
        &document(),
        &providers,
        &readers,
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
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let phase = FakeProvenancePhase::new().with_existing(false);
    let mut reporter = RecordingReporter::new();

    let error = release_provenance(
        &request(root.path()),
        &document(),
        &providers,
        &readers,
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
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let phase = FakeProvenancePhase::new();
    let mut reporter = RecordingReporter::new();

    let error = release_provenance(
        &request(root.path()),
        &document(),
        &providers,
        &readers,
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
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let phase = FakeProvenancePhase::new();
    let image = FakeImagePhase::new().with_existing_digest("sha256:img");
    let mut reporter = RecordingReporter::new();

    let report = release_provenance(
        &request(root.path()),
        &document(),
        &providers,
        &readers,
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
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let phase = FakeProvenancePhase::new();
    let image = FakeImagePhase::new().with_existing_digest("sha256:img");
    let mut reporter = RecordingReporter::new();

    let report = release_provenance(
        &request(root.path()),
        &document(),
        &providers,
        &readers,
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
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let phase = FakeProvenancePhase::new();
    let image =
        FakeImagePhase::new().with_reference_digest("docker.io/acme/toven:1.0.0", "sha256:img");
    let mut reporter = RecordingReporter::new();

    let error = release_provenance(
        &request(root.path()),
        &document(),
        &providers,
        &readers,
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
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let phase = FakeProvenancePhase::new();
    // A phase whose references resolve no digest: the image was never pushed.
    let image = FakeImagePhase::new();
    let mut reporter = RecordingReporter::new();

    let error = release_provenance(
        &request(root.path()),
        &document(),
        &providers,
        &readers,
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
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let phase = FakeProvenancePhase::failing("gh attestation missing");
    let image = FakeImagePhase::new();
    let mut reporter = RecordingReporter::new();

    let error = release_provenance(
        &request(root.path()),
        &document(),
        &providers,
        &readers,
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
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let phase = FakeProvenancePhase::new();
    let image = FakeImagePhase::new();
    let mut reporter = RecordingReporter::new();

    let error = release_provenance(
        &request(root.path()),
        &document(),
        &providers,
        &readers,
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
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    // The archive is attested; the SBOM is not.
    let phase = FakeProvenancePhase::new().with_missing("toven-sbom.cdx.json");
    let mut reporter = RecordingReporter::new();

    let report = release_provenance(
        &request(root.path()),
        &document(),
        &providers,
        &readers,
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
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let phase = FakeProvenancePhase::new();
    let mut reporter = RecordingReporter::new();

    let error = release_provenance(
        &request(root.path()),
        &document(),
        &providers,
        &readers,
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

#[derive(Default)]
struct Recorder {
    started: Vec<String>,
    settled: Vec<(String, toven_runtime::UnitStatus, Option<ProvenanceOutcome>)>,
}

impl toven_runtime::Progress<ProvenanceOutcome> for Recorder {
    fn started(&mut self, unit_id: &str) -> rskit_errors::AppResult<()> {
        self.started.push(unit_id.to_string());
        Ok(())
    }

    fn settled(
        &mut self,
        report: &toven_runtime::UnitReport<ProvenanceOutcome>,
    ) -> rskit_errors::AppResult<()> {
        self.settled.push((
            report.unit_id.clone(),
            report.status,
            report.outcome.clone(),
        ));
        Ok(())
    }
}

#[test]
fn provenance_units_are_edgeless_one_per_published_subject() {
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
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let phase: Arc<dyn ProvenancePhase> = Arc::new(FakeProvenancePhase::new());
    let mut reporter = RecordingReporter::new();

    let (_op, units) = provenance_operation(
        &request(root.path()),
        &document(),
        &providers,
        &readers,
        phase,
        &FakeImagePhase::new(),
        ProvenanceOptions::default(),
        &mut reporter,
    )
    .expect("provenance gather succeeds");

    assert_eq!(units.len(), 2);
    assert!(units.iter().all(|unit| unit.depends_on.is_empty()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provenance_streams_a_verified_subject_per_unit() {
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
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let phase: Arc<dyn ProvenancePhase> = Arc::new(FakeProvenancePhase::new());
    let mut reporter = RecordingReporter::new();

    let (op, units) = provenance_operation(
        &request(root.path()),
        &document(),
        &providers,
        &readers,
        phase,
        &FakeImagePhase::new(),
        ProvenanceOptions::default(),
        &mut reporter,
    )
    .expect("provenance gather succeeds");

    let mut rec = Recorder::default();
    let summary = toven_runtime::execute(
        &units,
        op,
        toven_runtime::EngineConfig {
            jobs: 2,
            fail_fast: false,
        },
        &mut rec,
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(summary.total, 2);
    assert_eq!(summary.succeeded, 2);
    assert!(!summary.has_failures());
    assert_eq!(rec.started.len(), 2);
    assert_eq!(rec.settled.len(), 2);
    assert!(
        rec.settled
            .iter()
            .all(|(_, status, _)| *status == toven_runtime::UnitStatus::Succeeded)
    );
    assert!(rec.settled.iter().all(|(_, _, outcome)| {
        outcome
            .as_ref()
            .is_some_and(|o| o.status == ProvenancePhaseStatus::Verified)
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provenance_stream_fails_a_subject_lacking_an_attestation() {
    let root = TempDir::new().unwrap();
    write_manifest(root.path(), &[("e".repeat(64).as_str(), "toven.tar.gz")]);
    let provider = provider_with_assets(vec!["dist/toven.tar.gz", "dist/SHA256SUMS"]);
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let phase: Arc<dyn ProvenancePhase> = Arc::new(FakeProvenancePhase::new().with_existing(false));
    let mut reporter = RecordingReporter::new();

    let (op, units) = provenance_operation(
        &request(root.path()),
        &document(),
        &providers,
        &readers,
        phase,
        &FakeImagePhase::new(),
        ProvenanceOptions::default(),
        &mut reporter,
    )
    .expect("provenance gather succeeds");

    let mut rec = Recorder::default();
    let summary = toven_runtime::execute(
        &units,
        op,
        toven_runtime::EngineConfig {
            jobs: 2,
            fail_fast: false,
        },
        &mut rec,
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();

    // A missing attestation streams as a failed unit (fail-closed) rather than
    // aborting the run — the enforced CLI turns any failure into a non-zero exit.
    assert!(summary.has_failures());
    assert_eq!(summary.failed, 1);
    let (_id, status, outcome) = &rec.settled[0];
    assert_eq!(*status, toven_runtime::UnitStatus::Failed);
    assert_eq!(
        outcome.as_ref().unwrap().status,
        ProvenancePhaseStatus::Missing
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provenance_stream_preview_reports_presence_without_failing() {
    let root = TempDir::new().unwrap();
    write_manifest(root.path(), &[("c".repeat(64).as_str(), "toven.tar.gz")]);
    let provider = provider_with_assets(vec!["dist/toven.tar.gz", "dist/SHA256SUMS"]);
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let phase: Arc<dyn ProvenancePhase> = Arc::new(FakeProvenancePhase::new().with_existing(false));
    let mut reporter = RecordingReporter::new();

    let (op, units) = provenance_operation(
        &request(root.path()),
        &document(),
        &providers,
        &readers,
        phase,
        &FakeImagePhase::new(),
        ProvenanceOptions { dry_run: true },
        &mut reporter,
    )
    .expect("provenance gather succeeds");

    let mut rec = Recorder::default();
    let summary = toven_runtime::execute(
        &units,
        op,
        toven_runtime::EngineConfig {
            jobs: 2,
            fail_fast: false,
        },
        &mut rec,
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();

    // Preview never fails, even with a missing attestation: the subject settles
    // succeeded carrying a `missing` status.
    assert!(!summary.has_failures());
    assert_eq!(summary.succeeded, 1);
    let (_id, status, outcome) = &rec.settled[0];
    assert_eq!(*status, toven_runtime::UnitStatus::Succeeded);
    assert_eq!(
        outcome.as_ref().unwrap().status,
        ProvenancePhaseStatus::Missing
    );
}
