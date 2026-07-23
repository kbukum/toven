//! Mutation-free preview regression coverage.
//!
//! Every preview command — `release plan`, `release status`, `release
//! readiness`, and `release publish --dry-run` (rehearsal) — must observe the
//! repository and release targets without changing anything. These tests drive
//! each command over a real temp git repo (via [`GitScenario`]) with a
//! recording release-target double, then assert the tracked-file digests, local
//! refs, and target call log prove no mutation occurred: no manifest write, no
//! commit, no tag, no push, and no ecosystem `package`/`apply_release`/`publish`
//! call. Network-free and deterministic.

use std::collections::BTreeSet;

use toven_engine::config::{CanonicalRegistry, Document, load};
use toven_engine::federation::MemberVcsReaders;
use toven_engine::plan::{PlanRequest, Selection};
use toven_engine::release::{
    BumpOverrides, release_plan, release_readiness, release_rehearse, release_status,
};
use toven_engine::vcs::RskitGitVcs;
use toven_model::{AbsPath, EcosystemId, Module, ModuleRef, RepoPath};
use toven_ports::{
    BaselineSpec, CommonEcosystemConfig, DiscoverResponse, Provider, ReleaseConfig, TaskIntent,
};
use toven_testkit::git::{GitScenario, ref_map_at, worktree_digests};
use toven_testkit::{
    FakeConfiguredAdapter, FakeProvider, FakeReleaseTarget, RecordingReporter, ReleaseCall,
    TestWorkspace,
};

fn eid(id: &str) -> EcosystemId {
    EcosystemId::new(id).expect("valid ecosystem id")
}

/// A registry-publishing rust provider exposing one changed `core` module, plus
/// the shared recording target so a test can assert its call log.
fn registry_provider() -> (FakeProvider, FakeReleaseTarget) {
    let mut response = DiscoverResponse::new(eid("rust"));
    response.modules.push(Module::new(
        ModuleRef::new(eid("rust"), "core").expect("ref"),
        RepoPath::new(".").expect("root"),
    ));
    let common = CommonEcosystemConfig {
        release: ReleaseConfig {
            registry: Some("crates-io".into()),
            readiness: Some(vec!["clean-tree".into(), "registry-idempotent".into()]),
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

/// Build a single-repo project with a committed baseline tag and a later,
/// uncommitted-free change so a real release is pending.
fn pending_release_repo() -> (TestWorkspace, AbsPath, Document) {
    let ws = TestWorkspace::new("release-preview-mutation-free");
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

/// Every recorded call must be a read: no packaging, mutation, or publication.
fn assert_only_reads(calls: &[ReleaseCall]) {
    for call in calls {
        assert!(
            !matches!(
                call,
                ReleaseCall::Package(_)
                    | ReleaseCall::ApplyRelease { .. }
                    | ReleaseCall::Publish(_)
                    | ReleaseCall::Sbom { .. }
            ),
            "preview must not invoke a mutating target call: {call:?}"
        );
    }
}

#[test]
fn previews_leave_the_repository_and_targets_unmutated() {
    let (ws, root, document) = pending_release_repo();
    let (provider, target) = registry_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = RskitGitVcs::open(ws.path()).expect("open reader");
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("rust/core@0.1.0"));

    let before_files = worktree_digests(ws.path()).expect("before digests");
    let before_refs = ref_map_at(ws.path()).expect("before refs");

    // Drive each preview command through its public facade.
    let mut reporter = RecordingReporter::new();
    let plan = release_plan(
        &request(root.clone()),
        &document,
        &providers,
        &readers,
        &BumpOverrides::new(),
        &mut reporter,
    )
    .expect("plan");
    assert!(
        !plan.entries.is_empty(),
        "the changed module should produce a pending release entry"
    );

    release_status(
        &request(root.clone()),
        &document,
        &providers,
        &readers,
        &mut reporter,
    )
    .expect("status");
    release_readiness(
        &request(root.clone()),
        &document,
        &providers,
        &readers,
        &mut reporter,
    )
    .expect("readiness");
    // `publish --dry-run` with and without the local rehearsal (`--no-push`) mode.
    for no_push in [false, true] {
        release_rehearse(
            &request(root.clone()),
            &document,
            &providers,
            &readers,
            &BumpOverrides::new(),
            &mut reporter,
            no_push,
        )
        .expect("rehearse");
    }

    let after_files = worktree_digests(ws.path()).expect("after digests");
    let after_refs = ref_map_at(ws.path()).expect("after refs");

    assert_eq!(before_files, after_files, "no tracked file may change");
    assert_eq!(before_refs, after_refs, "no local ref may change");
    assert_only_reads(&target.calls());
}
