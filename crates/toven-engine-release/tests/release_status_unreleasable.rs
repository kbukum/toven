//! `release status` must skip modules that do not participate in the release.
//!
//! An excluded (or otherwise non-releasable) module is dropped before its
//! release target is queried, so a target that cannot resolve a version for a
//! never-released module — e.g. a Go module with no reachable tag — must not
//! fail the read-only status report. This regresses an ordering bug where the
//! version reads ran ahead of the publication-policy filter.

use std::collections::BTreeSet;

use toven_engine_core::config::{CanonicalRegistry, Document, load};
use toven_engine_core::federation::MemberVcsReaders;
use toven_engine_core::plan::{PlanRequest, Selection};
use toven_engine_core::vcs::RskitGitVcs;
use toven_engine_release::release_status;
use toven_model::{AbsPath, EcosystemId, Module, ModuleRef, RepoPath};
use toven_ports::{
    BaselineSpec, CommonEcosystemConfig, DiscoverResponse, Provider, ReleaseConfig, TaskIntent,
};
use toven_testkit::git::GitScenario;
use toven_testkit::{
    FakeConfiguredAdapter, FakeProvider, FakeReleaseTarget, RecordingReporter, TestWorkspace,
};

fn eid(id: &str) -> EcosystemId {
    EcosystemId::new(id).expect("valid ecosystem id")
}

/// A rust provider exposing one excluded `core` module whose release target
/// hard-fails every version read (as a never-released target would).
fn excluded_provider() -> FakeProvider {
    let mut response = DiscoverResponse::new(eid("rust"));
    response.modules.push(Module::new(
        ModuleRef::new(eid("rust"), "core").expect("ref"),
        RepoPath::new(".").expect("root"),
    ));
    let common = CommonEcosystemConfig {
        release: ReleaseConfig {
            exclude: Some(true),
            ..ReleaseConfig::default()
        },
        ..CommonEcosystemConfig::default()
    };
    let target = FakeReleaseTarget::new().with_version_read_failure("no reachable release tag");
    let adapter = FakeConfiguredAdapter::new(eid("rust"))
        .with_response(response)
        .with_common(common)
        .with_release_target(target);
    FakeProvider::new(eid("rust")).with_adapter(adapter)
}

#[test]
fn status_skips_an_excluded_module_without_reading_its_version() {
    let ws = TestWorkspace::new("release-status-unreleasable");
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

    let root = AbsPath::new(ws.path().to_path_buf()).expect("absolute root");
    let document = load(
        ws.path().join("toven.toml"),
        &BTreeSet::new(),
        &CanonicalRegistry::model(),
    )
    .expect("document loads")
    .document;
    let _: &Document = &document;

    let provider = excluded_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = RskitGitVcs::open(ws.path()).expect("open reader");
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("HEAD"));

    let request = PlanRequest::new("rel", "t", TaskIntent::resolve("release"), root)
        .with_selection(Selection::All);
    let mut reporter = RecordingReporter::new();

    let status = release_status(&request, &document, &providers, &readers, &mut reporter)
        .expect("status must not fail on an excluded module with an unreadable version");
    assert!(
        status.modules.is_empty(),
        "the excluded module must be omitted from status, got {:?}",
        status.modules
    );
}
