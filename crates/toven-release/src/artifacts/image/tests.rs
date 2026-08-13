use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use rskit_config::RawValue;
use rskit_errors::ErrorCode;
use serde_json::json;
use toven_model::{AbsPath, EcosystemId, Module, ModuleRef, RepoPath};
use toven_ports::{
    BaselineSpec, CommonEcosystemConfig, DiscoverResponse, ImageConfig, ImageOutcome, Provider,
    ReleaseConfig, TaskIntent,
};
use toven_testkit::{
    FakeConfiguredAdapter, FakeImagePhase, FakeProvider, FakeReleaseTarget, FakeVcsReader,
    RecordingReporter,
};

use super::buildx::{
    BuildxImagePhase, buildx_build_argv, buildx_push_argv, inspect_argv, parse_metadata_digest,
};
use super::phase::{ImageOptions, ImagePhaseStatus, release_image};
use rskit_version::semver::Version;
use toven_core::config::{Document, ProjectConfig, TovenConfig};
use toven_core::federation::MemberVcsReaders;
use toven_core::plan::PlanRequest;
use toven_ports::{ImagePhase, ImageRequest};
use toven_testkit::FakeToolRunner;

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

fn request() -> PlanRequest {
    PlanRequest::new(
        "r1",
        "demo",
        TaskIntent::resolve("release"),
        AbsPath::new("/repo").unwrap(),
    )
}

/// A provider whose module declares an image block (or not) and reports the
/// given declared version.
fn provider_with_image(image: Option<ImageConfig>, version: Version) -> FakeProvider {
    let mut response = DiscoverResponse::new(eid("rust"));
    response.modules = vec![module("app")];
    let common = CommonEcosystemConfig {
        release: ReleaseConfig {
            image,
            ..ReleaseConfig::default()
        },
        ..CommonEcosystemConfig::default()
    };
    let adapter = FakeConfiguredAdapter::new(eid("rust"))
        .with_response(response)
        .with_release_target(FakeReleaseTarget::new().with_declared_version(version))
        .with_common(common);
    FakeProvider::new(eid("rust")).with_adapter(adapter)
}

fn image_config() -> ImageConfig {
    ImageConfig {
        registry: "ghcr.io/acme".into(),
        mirrors: vec!["docker.io/acme".into()],
        name: "toven".into(),
        tag: Some("{version}".into()),
        context: Some("services/api".into()),
        dockerfile: None,
        sign: true,
    }
}

#[test]
fn image_builds_once_pushes_primary_and_mirrors_and_signs() {
    let provider = provider_with_image(Some(image_config()), Version::new(1, 2, 3));
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let phase = FakeImagePhase::new().with_digest("sha256:abc");
    let mut reporter = RecordingReporter::new();

    let report = release_image(
        &request(),
        &document(),
        &providers,
        &readers,
        &phase,
        ImageOptions::default(),
        &mut reporter,
    )
    .expect("image phase runs");

    assert!(!report.preview);
    assert_eq!(report.images.len(), 1);
    let outcome = &report.images[0];
    assert_eq!(outcome.status, ImagePhaseStatus::Pushed);
    assert_eq!(outcome.digest.as_deref(), Some("sha256:abc"));
    assert!(outcome.signed);
    assert_eq!(
        outcome.references,
        vec![
            "ghcr.io/acme/toven:1.2.3".to_string(),
            "docker.io/acme/toven:1.2.3".to_string(),
        ]
    );
    // Exactly one build/push call was recorded (build once, push everywhere).
    let calls = phase.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].request.registries,
        vec!["ghcr.io/acme", "docker.io/acme"]
    );
}

#[test]
fn image_dry_run_previews_without_pushing() {
    let provider = provider_with_image(Some(image_config()), Version::new(1, 0, 0));
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let phase = FakeImagePhase::new();
    let mut reporter = RecordingReporter::new();

    let report = release_image(
        &request(),
        &document(),
        &providers,
        &readers,
        &phase,
        ImageOptions { dry_run: true },
        &mut reporter,
    )
    .expect("preview runs");

    assert!(report.preview);
    assert_eq!(report.images[0].status, ImagePhaseStatus::WouldPush);
    // A preview never builds or pushes.
    assert!(phase.calls().is_empty(), "preview must not push");
}

#[test]
fn image_dry_run_reports_an_existing_tag() {
    let provider = provider_with_image(Some(image_config()), Version::new(1, 0, 0));
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let phase = FakeImagePhase::new().with_existing_digest("sha256:live");
    let mut reporter = RecordingReporter::new();

    let report = release_image(
        &request(),
        &document(),
        &providers,
        &readers,
        &phase,
        ImageOptions { dry_run: true },
        &mut reporter,
    )
    .expect("preview runs");

    assert_eq!(report.images[0].status, ImagePhaseStatus::AlreadyPresent);
    assert_eq!(report.images[0].digest.as_deref(), Some("sha256:live"));
    assert!(phase.calls().is_empty());
}

#[test]
fn image_dry_run_fails_closed_on_divergent_primary_and_mirror_digests() {
    let provider = provider_with_image(Some(image_config()), Version::new(1, 0, 0));
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let phase = FakeImagePhase::new()
        .with_reference_digest("ghcr.io/acme/toven:1.0.0", "sha256:primary")
        .with_reference_digest("docker.io/acme/toven:1.0.0", "sha256:mirror");
    let mut reporter = RecordingReporter::new();

    let error = release_image(
        &request(),
        &document(),
        &providers,
        &readers,
        &phase,
        ImageOptions { dry_run: true },
        &mut reporter,
    )
    .expect_err("divergent preview state must fail closed");

    assert!(error.to_string().contains("reconcile"), "{error}");
    assert!(phase.calls().is_empty(), "preview must not push");
}

#[test]
fn image_fails_closed_on_a_divergent_existing_tag() {
    let provider = provider_with_image(Some(image_config()), Version::new(2, 0, 0));
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let phase = FakeImagePhase::new()
        .with_digest("sha256:new")
        .with_existing_digest("sha256:old");
    let mut reporter = RecordingReporter::new();

    let error = release_image(
        &request(),
        &document(),
        &providers,
        &readers,
        &phase,
        ImageOptions::default(),
        &mut reporter,
    )
    .expect_err("a divergent tag must fail closed");
    assert!(error.to_string().contains("immutable"), "{error}");
}

#[test]
fn image_without_a_block_is_a_typed_error() {
    let provider = provider_with_image(None, Version::new(1, 0, 0));
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let phase = FakeImagePhase::new();
    let mut reporter = RecordingReporter::new();

    let error = release_image(
        &request(),
        &document(),
        &providers,
        &readers,
        &phase,
        ImageOptions::default(),
        &mut reporter,
    )
    .expect_err("no image block must be a typed error");
    assert!(
        error
            .to_string()
            .contains("no module declares an image phase"),
        "{error}"
    );
}

#[test]
fn buildx_push_argv_pushes_every_reference_and_targets_the_context() {
    let request = ImageRequest::new("services/api", "toven", "1.2.3")
        .with_registries(vec!["ghcr.io/acme".into(), "docker.io/acme".into()]);
    let argv = buildx_push_argv(&request, &request.references()).expect("argv");
    assert_eq!(argv[0], "buildx");
    assert!(argv.iter().any(|token| token == "--push"));
    assert!(argv.iter().any(|token| token == "ghcr.io/acme/toven:1.2.3"));
    assert!(
        argv.iter()
            .any(|token| token == "docker.io/acme/toven:1.2.3")
    );
    assert_eq!(argv.last().map(String::as_str), Some("services/api"));
}

#[test]
fn buildx_build_argv_captures_the_digest_without_pushing() {
    let request = ImageRequest::new("services/api", "toven", "1.2.3")
        .with_registries(vec!["ghcr.io/acme".into()]);
    let argv = buildx_build_argv(&request, &request.references(), Path::new("/tmp/meta.json"))
        .expect("argv");
    assert!(argv.iter().any(|token| token == "--metadata-file"));
    assert!(argv.iter().any(|token| token == "/tmp/meta.json"));
    assert!(argv.iter().any(|token| token == "--load"));
    assert!(
        !argv.iter().any(|token| token == "--push"),
        "the digest-capture build must not push"
    );
    assert!(argv.iter().any(|token| token == "ghcr.io/acme/toven:1.2.3"));
    assert_eq!(argv.last().map(String::as_str), Some("services/api"));
}

#[test]
fn parse_metadata_digest_reads_containerimage_digest() {
    let digest =
        parse_metadata_digest(r#"{"containerimage.digest":"sha256:abc"}"#).expect("digest present");
    assert_eq!(digest, "sha256:abc");
}

#[test]
fn parse_metadata_digest_fails_closed_without_a_digest() {
    assert!(parse_metadata_digest(r#"{"other":"value"}"#).is_err());
    assert!(parse_metadata_digest("not json").is_err());
}

#[test]
fn inspect_argv_reads_the_manifest_digest() {
    let argv = inspect_argv("ghcr.io/acme/toven:1.2.3");
    assert!(argv.iter().any(|token| token == "inspect"));
    assert!(argv.iter().any(|token| token == "ghcr.io/acme/toven:1.2.3"));
}

#[test]
fn image_outcome_maps_already_complete() {
    let provider = provider_with_image(Some(image_config()), Version::new(1, 0, 0));
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let phase = FakeImagePhase::new().with_outcome(ImageOutcome::AlreadyComplete);
    let mut reporter = RecordingReporter::new();

    let report = release_image(
        &request(),
        &document(),
        &providers,
        &readers,
        &phase,
        ImageOptions::default(),
        &mut reporter,
    )
    .expect("runs");
    assert_eq!(report.images[0].status, ImagePhaseStatus::AlreadyComplete);
}

#[test]
fn image_surfaces_a_publish_failure() {
    let provider = provider_with_image(Some(image_config()), Version::new(1, 0, 0));
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
    let phase = FakeImagePhase::failing("buildx missing");
    let mut reporter = RecordingReporter::new();

    let error = release_image(
        &request(),
        &document(),
        &providers,
        &readers,
        &phase,
        ImageOptions::default(),
        &mut reporter,
    )
    .expect_err("a publish failure must surface");
    assert!(error.to_string().contains("buildx missing"), "{error}");
}

#[test]
fn buildx_resolve_digest_invokes_docker_argv_first() {
    let runner = FakeToolRunner::new().with_stdout("sha256:abc\n");
    let phase = BuildxImagePhase::new(Arc::new(runner.clone()));

    let digest = phase
        .resolve_digest(Path::new("/repo"), "ghcr.io/acme/toven:1.2.3")
        .expect("digest resolves");

    assert_eq!(digest.as_deref(), Some("sha256:abc"));
    let requests = runner.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].argv,
        vec![
            "docker",
            "buildx",
            "imagetools",
            "inspect",
            "--format",
            "{{.Manifest.Digest}}",
            "ghcr.io/acme/toven:1.2.3",
        ]
    );
    assert!(requests[0].forward_env.is_empty());
    assert!(
        requests[0]
            .argv
            .iter()
            .all(|arg| !arg.contains("ghp_secret")),
        "argv leaked a token value: {:?}",
        requests[0].argv
    );
}

#[test]
fn buildx_resolve_digest_treats_manifest_unknown_as_absent() {
    let runner = FakeToolRunner::new()
        .with_exit_code(Some(1))
        .with_stderr("manifest unknown");
    let phase = BuildxImagePhase::new(Arc::new(runner));

    let digest = phase
        .resolve_digest(Path::new("/repo"), "ghcr.io/acme/toven:1.2.3")
        .expect("missing image is absence");

    assert_eq!(digest, None);
}

#[test]
fn buildx_resolve_digest_fails_closed_on_real_tool_failure() {
    let runner = FakeToolRunner::new()
        .with_exit_code(Some(1))
        .with_stderr("unauthorized");
    let phase = BuildxImagePhase::new(Arc::new(runner));

    let error = phase
        .resolve_digest(Path::new("/repo"), "ghcr.io/acme/toven:1.2.3")
        .expect_err("auth failures are not absence");

    assert_eq!(error.code(), ErrorCode::Internal);
    assert!(
        error.to_string().contains("docker exited with code 1"),
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
fn buildx_publish_build_failure_records_argv_first_and_maps_error() {
    let runner = FakeToolRunner::new()
        .with_exit_code(Some(1))
        .with_stderr("build failed");
    let phase = BuildxImagePhase::new(Arc::new(runner.clone()));
    let request = ImageRequest::new("services/api", "toven", "1.2.3")
        .with_dockerfile("services/api/Dockerfile")
        .with_registries(vec!["ghcr.io/acme".into(), "docker.io/acme".into()]);

    let error = phase
        .publish_image(Path::new("/repo"), &request)
        .expect_err("build failure aborts publish");

    assert_eq!(error.code(), ErrorCode::ExternalService);
    assert!(error.to_string().contains("image tool `docker`"), "{error}");
    let requests = runner.requests();
    assert_eq!(requests.len(), 1);
    let argv = &requests[0].argv;
    assert_eq!(&argv[0..3], &["docker", "buildx", "build"]);
    assert!(
        argv.windows(2)
            .any(|pair| pair == ["--metadata-file", argv[4].as_str()])
    );
    assert!(argv.iter().any(|arg| arg == "--load"));
    assert!(
        argv.windows(2)
            .any(|pair| pair == ["--file", "services/api/Dockerfile"])
    );
    assert!(
        argv.windows(2)
            .any(|pair| pair == ["--tag", "ghcr.io/acme/toven:1.2.3"])
    );
    assert!(
        argv.windows(2)
            .any(|pair| pair == ["--tag", "docker.io/acme/toven:1.2.3"])
    );
    assert_eq!(argv.last().map(String::as_str), Some("services/api"));
}
