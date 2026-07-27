//! Repeated-preview determinism regression coverage.
//!
//! A release preview is only reviewable when asking twice yields the same
//! answer: `release plan` and `release publish --dry-run` (rehearsal) must be
//! pure functions of the repository and release-target state. These tests drive
//! each preview twice over a real temp git repo (via [`GitScenario`]) and
//! assert byte-equal typed results, with the recording release-target double
//! proving repetition invoked no mutation.

use std::collections::BTreeSet;

use toven_engine::config::{CanonicalRegistry, Document, load};
use toven_engine::federation::MemberVcsReaders;
use toven_engine::plan::{PlanRequest, Selection};
use toven_engine::release::{BumpOverrides, release_plan, release_rehearse};
use toven_engine::vcs::RskitGitVcs;
use toven_model::{AbsPath, EcosystemId, Module, ModuleRef, RepoPath};
use toven_ports::{
    BaselineSpec, CommonEcosystemConfig, DiscoverResponse, Provider, ReleaseConfig, TaskIntent,
};
use toven_testkit::git::GitScenario;
use toven_testkit::{
    FakeConfiguredAdapter, FakeProvider, FakeReleaseTarget, RecordingReporter, ReleaseCall,
    TestWorkspace,
};

fn eid(id: &str) -> EcosystemId {
    EcosystemId::new(id).expect("valid ecosystem id")
}

/// A registry-publishing rust provider exposing one releasable `core` module,
/// plus the shared recording target so a test can assert its call log.
fn registry_provider() -> (FakeProvider, FakeReleaseTarget) {
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
    let target = FakeReleaseTarget::new();
    let adapter = FakeConfiguredAdapter::new(eid("rust"))
        .with_response(response)
        .with_common(common)
        .with_release_target(target.clone());
    (FakeProvider::new(eid("rust")).with_adapter(adapter), target)
}

/// Build a single-repo project with a committed baseline tag and a later
/// change so a real release is pending.
fn pending_release_repo() -> (TestWorkspace, AbsPath, Document) {
    let ws = TestWorkspace::new("release-plan-determinism");
    let scenario = GitScenario::init(ws.path()).expect("git init");
    scenario
        .commit_file(
            "toven.toml",
            "[project]\nname = \"t\"\n\n[ecosystems.rust]\n",
            "config",
        )
        .expect("commit config");
    scenario
        .commit_file("src/lib.rs", "pub fn a() {}\n", "baseline")
        .expect("baseline commit");
    scenario.tag("rust/core@0.1.0", "baseline").expect("tag");
    scenario
        .commit_file("src/lib.rs", "pub fn a() {}\npub fn b() {}\n", "feature")
        .expect("feature commit");

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
    PlanRequest::new("rel", "t", TaskIntent::resolve("release"), root).with_selection(
        Selection::Changed(Some(BaselineSpec::explicit("rust/core@0.1.0"))),
    )
}

#[test]
fn repeated_plans_are_identical_and_read_only() {
    let (ws, root, document) = pending_release_repo();
    let (provider, target) = registry_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = RskitGitVcs::open(ws.path()).expect("open reader");
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("rust/core@0.1.0"));

    let mut reporter = RecordingReporter::new();
    let first = release_plan(
        &request(root.clone()),
        &document,
        &providers,
        &readers,
        &BumpOverrides::new(),
        &mut reporter,
    )
    .expect("first plan");
    let second = release_plan(
        &request(root),
        &document,
        &providers,
        &readers,
        &BumpOverrides::new(),
        &mut reporter,
    )
    .expect("second plan");

    assert_eq!(first, second, "repeated plans must be identical");
    assert_eq!(first.entries.len(), 1);
    // The plan explains the tag a mutating run would create, deterministically.
    assert_eq!(
        first.entries[0].planned_tag.as_deref(),
        Some("rust/core@0.1.1")
    );
    assert!(
        target.calls().iter().all(|call| !matches!(
            call,
            ReleaseCall::Package(_) | ReleaseCall::ApplyRelease { .. } | ReleaseCall::Publish(_)
        )),
        "planning must stay read-only: {:?}",
        target.calls()
    );
}

#[test]
fn repeated_dry_run_rehearsals_are_identical_and_read_only() {
    let (ws, root, document) = pending_release_repo();
    let (provider, target) = registry_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = RskitGitVcs::open(ws.path()).expect("open reader");
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("rust/core@0.1.0"));

    let mut reporter = RecordingReporter::new();
    let first = release_rehearse(
        &request(root.clone()),
        &document,
        &providers,
        &readers,
        &BumpOverrides::new(),
        &mut reporter,
        false,
    )
    .expect("first rehearsal");
    let second = release_rehearse(
        &request(root),
        &document,
        &providers,
        &readers,
        &BumpOverrides::new(),
        &mut reporter,
        false,
    )
    .expect("second rehearsal");

    assert_eq!(first, second, "repeated rehearsals must be identical");
    assert!(
        target.calls().iter().all(|call| !matches!(
            call,
            ReleaseCall::Package(_) | ReleaseCall::ApplyRelease { .. } | ReleaseCall::Publish(_)
        )),
        "rehearsal must stay read-only: {:?}",
        target.calls()
    );
}
