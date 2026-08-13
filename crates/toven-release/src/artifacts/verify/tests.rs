use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use rskit_config::RawValue;
use rskit_errors::ErrorCode;
use rskit_fs::TempDir;
use rskit_fs::archive::{ArchiveEntry, tar_gz};
use rskit_version::semver::Version;
use serde_json::json;
use toven_model::{AbsPath, EcosystemId, Module, ModuleRef, RepoPath};
use toven_ports::{
    AssetDownloader, BaselineSpec, CommonEcosystemConfig, DiscoverResponse, HostConfig, Provider,
    ReleaseConfig, SignConfig, SignatureVerifier, TaskIntent, VersionProbe,
};
use toven_testkit::{
    FakeAssetDownloader, FakeConfiguredAdapter, FakeProvider, FakeReleaseTarget,
    FakeSignatureVerifier, FakeToolRunner, FakeVcsReader, FakeVersionProbe, RecordingReporter,
};

use super::assets::extract_binary;
use super::{
    CosignVerifier, GhAssetDownloader, ProcessVersionProbe, VerifyMode, VerifyOptions,
    release_verify,
};
use toven_core::config::{Document, ProjectConfig, TovenConfig};
use toven_core::federation::MemberVcsReaders;
use toven_core::plan::PlanRequest;

const LINUX_ARCHIVE: &str = "dist/toven-x86_64-unknown-linux-gnu.tar.gz";
const ARCHIVE_NAME: &str = "toven-x86_64-unknown-linux-gnu.tar.gz";
const VERSION: &str = "0.4.2";

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

/// A provider whose ecosystem declares `assets` and the given `sign` config,
/// with a release target reporting `VERSION`.
fn provider(assets: Vec<&str>, sign: SignConfig) -> FakeProvider {
    let mut response = DiscoverResponse::new(eid("rust"));
    response.modules = vec![module("cli")];
    let common = CommonEcosystemConfig {
        release: ReleaseConfig {
            host: Some(HostConfig {
                forge: Some("github".to_string()),
                assets: Some(assets.into_iter().map(str::to_string).collect()),
                ..HostConfig::default()
            }),
            sign: Some(sign),
            ..ReleaseConfig::default()
        },
        ..CommonEcosystemConfig::default()
    };
    let target = FakeReleaseTarget::new().with_declared_version(Version::parse(VERSION).unwrap());
    let adapter = FakeConfiguredAdapter::new(eid("rust"))
        .with_response(response)
        .with_release_target(target)
        .with_common(common);
    FakeProvider::new(eid("rust")).with_adapter(adapter)
}

/// Write a deterministic `toven` archive at `root/dist/<ARCHIVE_NAME>`
/// carrying a single binary member named `toven`.
fn write_archive(root: &Path) {
    let dir = root.join("dist");
    std::fs::create_dir_all(&dir).unwrap();
    let binary = dir.join("toven-binary");
    std::fs::write(&binary, b"fake-toven").unwrap();
    let entries = [ArchiveEntry::new("toven", &binary, 0o755)];
    tar_gz(&entries, &dir.join(ARCHIVE_NAME)).unwrap();
    std::fs::remove_file(&binary).unwrap();
}

#[test]
fn local_verify_presence_and_version() {
    let root = TempDir::new().unwrap();
    write_archive(root.path());
    let provider = provider(vec![LINUX_ARCHIVE], SignConfig::default());
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let mut reporter = RecordingReporter::new();
    let downloader = FakeAssetDownloader::from_dir(root.path());
    let verifier = FakeSignatureVerifier::new();
    let probe = FakeVersionProbe::reporting(format!("toven {VERSION}"));

    let report = release_verify(
        &request(root.path()),
        &document(),
        &providers,
        &readers,
        VerifyOptions {
            download: false,
            run: true,
        },
        &downloader,
        &verifier,
        &probe,
        &mut reporter,
    )
    .unwrap();

    assert_eq!(report.mode, VerifyMode::Local);
    assert_eq!(report.expected_version, VERSION);
    assert_eq!(report.assets.len(), 1);
    assert_eq!(report.assets[0].name, ARCHIVE_NAME);
    assert!(report.assets[0].ran);
    assert_eq!(
        report.assets[0].reported_version.as_deref(),
        Some(&*format!("toven {VERSION}"))
    );
    assert_eq!(report.assets[0].checksum_ok, None);
}

#[test]
fn local_no_run_skips_execution_but_checks_presence() {
    let root = TempDir::new().unwrap();
    write_archive(root.path());
    let provider = provider(vec![LINUX_ARCHIVE], SignConfig::default());
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let mut reporter = RecordingReporter::new();
    let downloader = FakeAssetDownloader::from_dir(root.path());
    let verifier = FakeSignatureVerifier::new();
    let probe = FakeVersionProbe::failing("must not run");

    let report = release_verify(
        &request(root.path()),
        &document(),
        &providers,
        &readers,
        VerifyOptions {
            download: false,
            run: false,
        },
        &downloader,
        &verifier,
        &probe,
        &mut reporter,
    )
    .unwrap();

    assert!(!report.assets[0].ran);
    assert!(report.assets[0].reported_version.is_none());
    assert!(probe.probed().is_empty(), "the binary must not be executed");
}

#[test]
fn local_verify_fails_closed_on_missing_archive() {
    let root = TempDir::new().unwrap();
    // No archive written.
    let provider = provider(vec![LINUX_ARCHIVE], SignConfig::default());
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let mut reporter = RecordingReporter::new();
    let downloader = FakeAssetDownloader::from_dir(root.path());
    let verifier = FakeSignatureVerifier::new();
    let probe = FakeVersionProbe::reporting(format!("toven {VERSION}"));

    let error = release_verify(
        &request(root.path()),
        &document(),
        &providers,
        &readers,
        VerifyOptions {
            download: false,
            run: true,
        },
        &downloader,
        &verifier,
        &probe,
        &mut reporter,
    )
    .expect_err("a missing archive must fail closed");
    assert!(error.to_string().contains("is not present"), "{error}");
}

#[test]
fn local_verify_fails_closed_on_wrong_reported_version() {
    let root = TempDir::new().unwrap();
    write_archive(root.path());
    let provider = provider(vec![LINUX_ARCHIVE], SignConfig::default());
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let mut reporter = RecordingReporter::new();
    let downloader = FakeAssetDownloader::from_dir(root.path());
    let verifier = FakeSignatureVerifier::new();
    let probe = FakeVersionProbe::reporting("toven 9.9.9");

    let error = release_verify(
        &request(root.path()),
        &document(),
        &providers,
        &readers,
        VerifyOptions {
            download: false,
            run: true,
        },
        &downloader,
        &verifier,
        &probe,
        &mut reporter,
    )
    .expect_err("a wrong reported version must fail closed");
    assert!(
        error.to_string().contains("expected 'toven 0.4.2'"),
        "{error}"
    );
}

/// Stage a "remote" directory holding the archive, a correct `SHA256SUMS`,
/// and signature/certificate sidecars, returning its path (kept alive by the
/// returned `TempDir`).
fn stage_remote(tamper: bool) -> (TempDir, String) {
    let remote = TempDir::new().unwrap();
    let binary = remote.path().join("toven-binary");
    std::fs::write(&binary, b"fake-toven").unwrap();
    let entries = [ArchiveEntry::new("toven", &binary, 0o755)];
    tar_gz(&entries, &remote.path().join(ARCHIVE_NAME)).unwrap();
    std::fs::remove_file(&binary).unwrap();

    let digest = if tamper {
        "0".repeat(64)
    } else {
        let mut file = std::fs::File::open(remote.path().join(ARCHIVE_NAME)).unwrap();
        rskit_util::hash::sha256::sha256_reader(&mut file)
            .unwrap()
            .to_hex()
    };
    std::fs::write(
        remote.path().join("SHA256SUMS"),
        format!("{digest}  {ARCHIVE_NAME}\n"),
    )
    .unwrap();
    std::fs::write(remote.path().join("SHA256SUMS.sig"), b"sig").unwrap();
    std::fs::write(remote.path().join("SHA256SUMS.pem"), b"pem").unwrap();
    let path = remote.path().to_str().unwrap().to_string();
    (remote, path)
}

fn signed_provider() -> FakeProvider {
    let sign = SignConfig {
        enabled: true,
        signer: None,
        identity: Some(
            "https://github.com/kbukum/toven/.github/workflows/release.yml@.*".to_string(),
        ),
        issuer: Some("https://token.actions.githubusercontent.com".to_string()),
    };
    provider(vec![LINUX_ARCHIVE], sign)
}

#[test]
fn download_verify_signature_then_checksum_then_run() {
    let (_remote, remote_path) = stage_remote(false);
    let root = TempDir::new().unwrap();
    let provider = signed_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let mut reporter = RecordingReporter::new();
    let downloader = FakeAssetDownloader::from_dir(remote_path);
    let verifier = FakeSignatureVerifier::new();
    let probe = FakeVersionProbe::reporting(format!("toven {VERSION}"));

    let report = release_verify(
        &request(root.path()),
        &document(),
        &providers,
        &readers,
        VerifyOptions {
            download: true,
            run: true,
        },
        &downloader,
        &verifier,
        &probe,
        &mut reporter,
    )
    .unwrap();

    assert_eq!(report.mode, VerifyMode::Download);
    assert_eq!(report.tag.as_deref(), Some(&*format!("v{VERSION}")));
    assert_eq!(report.assets[0].checksum_ok, Some(true));
    assert_eq!(report.assets[0].signature_ok, Some(true));
    assert!(report.assets[0].ran);
    // The signature was verified against the configured keyless identity.
    assert_eq!(verifier.calls().len(), 1);
    assert!(
        verifier.calls()[0]
            .issuer
            .contains("token.actions.githubusercontent.com")
    );
}

#[test]
fn download_verify_matches_checksum_with_uppercase_manifest_hex() {
    // A `SHA256SUMS` written with uppercase hex must still match the
    // lowercase digest we compute — `parse_manifest` normalizes to lowercase,
    // so an uppercase manifest is not a false negative.
    let (remote, remote_path) = stage_remote(false);
    let manifest = remote.path().join("SHA256SUMS");
    let body = std::fs::read_to_string(&manifest).unwrap();
    let (hex, name) = body.trim_end().split_once("  ").unwrap();
    std::fs::write(&manifest, format!("{}  {name}\n", hex.to_uppercase())).unwrap();

    let root = TempDir::new().unwrap();
    let provider = signed_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let mut reporter = RecordingReporter::new();
    let downloader = FakeAssetDownloader::from_dir(remote_path);
    let verifier = FakeSignatureVerifier::new();
    let probe = FakeVersionProbe::reporting(format!("toven {VERSION}"));

    let report = release_verify(
        &request(root.path()),
        &document(),
        &providers,
        &readers,
        VerifyOptions {
            download: true,
            run: true,
        },
        &downloader,
        &verifier,
        &probe,
        &mut reporter,
    )
    .unwrap();

    assert_eq!(report.assets[0].checksum_ok, Some(true));
}

#[test]
fn download_verify_aborts_before_checksum_on_bad_signature() {
    let (_remote, remote_path) = stage_remote(false);
    let root = TempDir::new().unwrap();
    let provider = signed_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let mut reporter = RecordingReporter::new();
    let downloader = FakeAssetDownloader::from_dir(remote_path);
    let verifier = FakeSignatureVerifier::failing("signature does not verify");
    let probe = FakeVersionProbe::failing("must not run");

    let error = release_verify(
        &request(root.path()),
        &document(),
        &providers,
        &readers,
        VerifyOptions {
            download: true,
            run: true,
        },
        &downloader,
        &verifier,
        &probe,
        &mut reporter,
    )
    .expect_err("a bad signature must abort");
    assert!(error.to_string().contains("does not verify"), "{error}");
    assert!(
        probe.probed().is_empty(),
        "must not extract/run after a bad signature"
    );
}

#[test]
fn download_verify_fails_closed_on_tampered_checksum() {
    let (_remote, remote_path) = stage_remote(true);
    let root = TempDir::new().unwrap();
    let provider = signed_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let mut reporter = RecordingReporter::new();
    let downloader = FakeAssetDownloader::from_dir(remote_path);
    let verifier = FakeSignatureVerifier::new();
    let probe = FakeVersionProbe::failing("must not run");

    let error = release_verify(
        &request(root.path()),
        &document(),
        &providers,
        &readers,
        VerifyOptions {
            download: true,
            run: true,
        },
        &downloader,
        &verifier,
        &probe,
        &mut reporter,
    )
    .expect_err("a tampered checksum must fail closed");
    assert!(error.to_string().contains("checksum mismatch"), "{error}");
    assert!(
        probe.probed().is_empty(),
        "must not run a checksum-failing archive"
    );
}

#[test]
fn download_no_run_still_verifies_signature_and_checksum() {
    let (_remote, remote_path) = stage_remote(false);
    let root = TempDir::new().unwrap();
    let provider = signed_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let mut reporter = RecordingReporter::new();
    let downloader = FakeAssetDownloader::from_dir(remote_path);
    let verifier = FakeSignatureVerifier::new();
    let probe = FakeVersionProbe::failing("must not run");

    let report = release_verify(
        &request(root.path()),
        &document(),
        &providers,
        &readers,
        VerifyOptions {
            download: true,
            run: false,
        },
        &downloader,
        &verifier,
        &probe,
        &mut reporter,
    )
    .unwrap();

    assert_eq!(report.assets[0].checksum_ok, Some(true));
    assert_eq!(report.assets[0].signature_ok, Some(true));
    assert!(!report.assets[0].ran);
    assert_eq!(verifier.calls().len(), 1);
    assert!(probe.probed().is_empty());
}

#[test]
fn download_fails_closed_when_identity_unconfigured() {
    let (_remote, remote_path) = stage_remote(false);
    let root = TempDir::new().unwrap();
    // Signing enabled but no identity/issuer configured.
    let sign = SignConfig {
        enabled: true,
        ..SignConfig::default()
    };
    let provider = provider(vec![LINUX_ARCHIVE], sign);
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let mut reporter = RecordingReporter::new();
    let downloader = FakeAssetDownloader::from_dir(remote_path);
    let verifier = FakeSignatureVerifier::new();
    let probe = FakeVersionProbe::reporting(format!("toven {VERSION}"));

    let error = release_verify(
        &request(root.path()),
        &document(),
        &providers,
        &readers,
        VerifyOptions {
            download: true,
            run: true,
        },
        &downloader,
        &verifier,
        &probe,
        &mut reporter,
    )
    .expect_err("download verification needs a configured identity");
    assert!(error.to_string().contains("keyless identity"), "{error}");
}

#[test]
fn extract_binary_fails_closed_on_multiple_members() {
    let root = TempDir::new().unwrap();
    let dir = root.path().join("dist");
    std::fs::create_dir_all(&dir).unwrap();
    let first = dir.join("toven-binary");
    std::fs::write(&first, b"the-binary").unwrap();
    let extra = dir.join("extra-file");
    std::fs::write(&extra, b"stowaway").unwrap();
    let archive = dir.join("multi.tar.gz");
    tar_gz(
        &[
            ArchiveEntry::new("toven", &first, 0o755),
            ArchiveEntry::new("extra", &extra, 0o644),
        ],
        &archive,
    )
    .unwrap();

    let dest = TempDir::new().unwrap();
    let error = extract_binary(&archive, dest.path())
        .expect_err("a multi-member archive must fail closed, not run the first member");
    assert!(
        error.to_string().contains("more than one member"),
        "{error}"
    );
}

#[test]
fn gh_downloader_builds_argv_first_without_token_values() {
    let dest = TempDir::new().unwrap();
    let runner = FakeToolRunner::new();
    let downloader = GhAssetDownloader::new(Arc::new(runner.clone()));

    downloader
        .download("v1.2.3", &["toven.tar.gz", "SHA256SUMS"], dest.path())
        .expect("download runs");

    let requests = runner.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(
        request.argv,
        vec![
            "gh",
            "release",
            "download",
            "v1.2.3",
            "--dir",
            dest.path().to_str().unwrap(),
            "--pattern",
            "toven.tar.gz",
            "--pattern",
            "SHA256SUMS",
        ]
    );
    assert!(request.forward_env.is_empty());
    assert!(
        request.argv.iter().all(|arg| !arg.contains("ghp_secret")),
        "argv leaked a token value: {:?}",
        request.argv
    );
}

#[test]
fn gh_downloader_non_zero_exit_is_a_verify_tool_error() {
    let dest = TempDir::new().unwrap();
    let runner = FakeToolRunner::new()
        .with_exit_code(Some(1))
        .with_stderr("release not found");
    let downloader = GhAssetDownloader::new(Arc::new(runner));

    let error = downloader
        .download("v1.2.3", &["toven.tar.gz"], dest.path())
        .expect_err("non-zero gh exits fail closed");

    assert_eq!(error.code(), ErrorCode::ExternalService);
    assert!(error.to_string().contains("verify tool `gh`"), "{error}");
    assert!(error.to_string().contains("exited 1"), "{error}");
}

#[test]
fn cosign_verifier_builds_argv_first() {
    let root = TempDir::new().unwrap();
    let blob = root.path().join("SHA256SUMS");
    let signature = root.path().join("SHA256SUMS.sig");
    let certificate = root.path().join("SHA256SUMS.pem");
    let runner = FakeToolRunner::new();
    let verifier = CosignVerifier::new(Arc::new(runner.clone()));

    verifier
        .verify_blob(
            &blob,
            &signature,
            &certificate,
            "https://github.com/acme/toven/.github/workflows/release.yml@refs/tags/v.*",
            "https://issuer.example",
        )
        .expect("verification runs");

    let requests = runner.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].argv,
        vec![
            "cosign",
            "verify-blob",
            "--certificate",
            certificate.to_str().unwrap(),
            "--signature",
            signature.to_str().unwrap(),
            "--certificate-identity-regexp",
            "https://github.com/acme/toven/.github/workflows/release.yml@refs/tags/v.*",
            "--certificate-oidc-issuer",
            "https://issuer.example",
            blob.to_str().unwrap(),
        ]
    );
    assert!(requests[0].forward_env.is_empty());
}

#[test]
fn cosign_verifier_spawn_failure_surfaces_as_typed_error() {
    let root = TempDir::new().unwrap();
    let runner = FakeToolRunner::new().with_spawn_failure("cosign not found");
    let verifier = CosignVerifier::new(Arc::new(runner));

    let error = verifier
        .verify_blob(
            &root.path().join("SHA256SUMS"),
            &root.path().join("SHA256SUMS.sig"),
            &root.path().join("SHA256SUMS.pem"),
            "identity",
            "issuer",
        )
        .expect_err("spawn failure surfaces");

    assert_eq!(error.code(), ErrorCode::Internal);
    assert!(error.to_string().contains("cosign not found"), "{error}");
}

#[test]
fn version_probe_runs_binary_version_argv_first() {
    let root = TempDir::new().unwrap();
    let binary = root.path().join("toven");
    let runner = FakeToolRunner::new().with_stdout("toven 1.2.3\n");
    let probe = ProcessVersionProbe::new(Arc::new(runner.clone()));

    let reported = probe.report_version(&binary).expect("version runs");

    assert_eq!(reported, "toven 1.2.3\n");
    let requests = runner.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].argv,
        vec![binary.to_str().unwrap(), "--version"]
    );
}
