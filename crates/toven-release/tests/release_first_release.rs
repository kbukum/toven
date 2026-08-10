//! First-release (tagless repository) planning coverage.
//!
//! A repository that has never cut a release has no release tag to diff
//! against. The release baseline must then report every releaseable module as
//! an initial release rather than silently diffing against a branch ref such as
//! `[project].base_ref` — which, on the very branch being released from,
//! reports no changes and would plan an empty first release.

use std::collections::BTreeSet;

use toven_core::config::{CanonicalRegistry, Document, load};
use toven_core::federation::MemberVcsReaders;
use toven_core::plan::PlanRequest;
use toven_core::vcs::RskitGitVcs;
use toven_model::{AbsPath, EcosystemId, Module, ModuleRef, RepoPath};
use toven_ports::{
    BaselineSpec, CommonEcosystemConfig, DiscoverResponse, Provider, ReleaseConfig, TaskIntent,
};
use toven_release::{BumpOverrides, BumpReason, release_plan};
use toven_testkit::git::GitScenario;
use toven_testkit::{
    FakeConfiguredAdapter, FakeProvider, FakeReleaseTarget, RecordingReporter, TestWorkspace,
};

fn eid(id: &str) -> EcosystemId {
    EcosystemId::new(id).expect("valid ecosystem id")
}

/// A registry-publishing rust provider exposing one releaseable `core` module.
fn registry_provider() -> FakeProvider {
    let mut response = DiscoverResponse::new(eid("rust"));
    response.modules.push(Module::new(
        ModuleRef::new(eid("rust"), "core").expect("ref"),
        RepoPath::new(".").expect("root"),
    ));
    let common = CommonEcosystemConfig {
        release: ReleaseConfig {
            registry: Some("crates-io".into()),
            ..ReleaseConfig::default()
        },
        ..CommonEcosystemConfig::default()
    };
    let adapter = FakeConfiguredAdapter::new(eid("rust"))
        .with_response(response)
        .with_common(common)
        .with_release_target(FakeReleaseTarget::new());
    FakeProvider::new(eid("rust")).with_adapter(adapter)
}

/// A committed repository that carries a `base_ref` but no release tag at all.
fn tagless_repo() -> (TestWorkspace, AbsPath, Document) {
    let ws = TestWorkspace::new("release-first-release");
    let scenario = GitScenario::init(ws.path()).expect("git init");
    scenario
        .commit_file(
            "toven.toml",
            "[project]\nname = \"t\"\nbase_ref = \"HEAD\"\n\n[ecosystems.rust]\n",
            "config",
        )
        .expect("commit config");
    scenario
        .commit_file("src/lib.rs", "pub fn a() {}\n", "initial source")
        .expect("source commit");

    let root = AbsPath::new(ws.path().to_path_buf()).expect("absolute root");
    let document = load(
        ws.path().join("toven.toml"),
        &BTreeSet::new(),
        &CanonicalRegistry::model(),
    )
    .expect("document loads")
    .document;
    (ws, root, document)
}

fn request(root: AbsPath) -> PlanRequest {
    PlanRequest::new("rel", "t", TaskIntent::resolve("release"), root)
}

#[test]
fn a_repository_with_no_release_tag_plans_an_initial_release() {
    let (ws, root, document) = tagless_repo();
    let provider = registry_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = RskitGitVcs::open(ws.path()).expect("open reader");
    // `HEAD` is the configured baseline: diffing against it yields no changes,
    // so an empty plan here would mean the branch ref leaked into the release
    // baseline.
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("HEAD"));

    let mut reporter = RecordingReporter::new();
    let plan = release_plan(
        &request(root),
        &document,
        &providers,
        &readers,
        &BumpOverrides::new(),
        &mut reporter,
    )
    .expect("plan");

    assert_eq!(
        plan.entries.len(),
        1,
        "a never-released module must join the first release: {:?}",
        plan.entries
    );
    let entry = &plan.entries[0];
    assert_eq!(
        entry.planned_version.as_ref(),
        Some(&entry.current_version),
        "a first release cuts the declared version rather than bumping past it: {entry:?}"
    );
    assert_eq!(entry.reason, BumpReason::InitialRelease);
    assert!(
        !entry.up_to_date,
        "a never-released module is not up to date"
    );
    let baseline = entry.baseline.as_ref().expect("initial baseline recorded");
    assert!(baseline.is_initial(), "expected an initial baseline");
    assert_eq!(baseline.tag, None);
    assert_eq!(
        entry.changelog.summary, "initial release",
        "a first release is not a dependency cascade"
    );
}

#[test]
fn a_base_override_does_not_downgrade_a_tagless_module_to_an_empty_release() {
    // `--base` overrides the diff ref only when a release tag anchors it. On a
    // tagless module it must be ignored: honoring it (here, `HEAD`) would diff
    // against a ref that reports no changes and silently plan an empty first
    // release instead of the initial release every unreleased module deserves.
    let (ws, root, document) = tagless_repo();
    let provider = registry_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = RskitGitVcs::open(ws.path()).expect("open reader");
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("HEAD"));

    let mut reporter = RecordingReporter::new();
    let plan = release_plan(
        &request(root),
        &document,
        &providers,
        &readers,
        &BumpOverrides::new().with_base("HEAD"),
        &mut reporter,
    )
    .expect("plan");

    assert_eq!(
        plan.entries.len(),
        1,
        "`--base` must not suppress a never-released module's first release: {:?}",
        plan.entries
    );
    let entry = &plan.entries[0];
    assert_eq!(entry.reason, BumpReason::InitialRelease);
    let baseline = entry.baseline.as_ref().expect("initial baseline recorded");
    assert!(
        baseline.is_initial(),
        "a tagless module stays an initial release even under `--base`"
    );
}
