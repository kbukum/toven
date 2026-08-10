//! Umbrella-tag + registry-anchored change-gating (the rskit `From == To` fix).
//!
//! A Rust workspace can carry per-crate release tag schemes
//! (`rust/<crate>@<version>`) yet cut only a single shared umbrella tag
//! (`v<version>`) and let crates.io be the per-crate registry. The old
//! own-tag-only baseline found no `rust/<crate>@…` tag for any crate, treated
//! every crate as an initial release, and planned `From == To` — bumping
//! nothing. Anchoring the baseline on the umbrella tag's commit and the
//! registry's max published version instead makes exactly the crates that
//! changed since the umbrella tag bump, and leaves the rest untouched.

use std::collections::BTreeSet;

use rskit_version::semver::Version;
use toven_engine_core::config::{CanonicalRegistry, Document, load};
use toven_engine_core::federation::MemberVcsReaders;
use toven_engine_core::plan::PlanRequest;
use toven_engine_core::vcs::RskitGitVcs;
use toven_engine_release::{BumpOverrides, BumpReason, release_plan};
use toven_model::{AbsPath, EcosystemId, Module, ModuleRef, RepoPath};
use toven_ports::{
    BaselineSpec, CommonEcosystemConfig, DiscoverResponse, Provider, ReleaseConfig, TaskIntent,
};
use toven_testkit::git::GitScenario;
use toven_testkit::{
    FakeConfiguredAdapter, FakeProvider, FakeReleaseTarget, RecordingReporter, TestWorkspace,
};

fn eid() -> EcosystemId {
    EcosystemId::new("rust").expect("valid ecosystem id")
}

fn mref(name: &str) -> ModuleRef {
    ModuleRef::new(eid(), name).expect("valid module ref")
}

/// A rust provider exposing an umbrella suite module plus two per-crate modules,
/// each with the default per-crate `rust/<crate>@` tag scheme. The registry
/// reports `0.1.0` published for every crate, and each crate declares `0.1.0`.
fn umbrella_provider() -> FakeProvider {
    // crates.io reports `0.1.0` already published for every crate — the same
    // version the single umbrella tag denotes — so the baseline anchors there,
    // never treating a crate as a first release.
    umbrella_provider_with(
        FakeReleaseTarget::new()
            .with_declared_version(Version::new(0, 1, 0))
            .with_published_versions(vec![Version::new(0, 1, 0)]),
    )
}

/// The three-module umbrella provider around a scripted release `target`.
fn umbrella_provider_with(target: FakeReleaseTarget) -> FakeProvider {
    let mut response = DiscoverResponse::new(eid());
    response.modules.push(Module::new(
        mref("suite"),
        RepoPath::new("crates/suite").expect("suite path"),
    ));
    response.modules.push(Module::new(
        mref("core"),
        RepoPath::new("crates/core").expect("core path"),
    ));
    response.modules.push(Module::new(
        mref("util"),
        RepoPath::new("crates/util").expect("util path"),
    ));
    let common = CommonEcosystemConfig {
        release: ReleaseConfig {
            registry: Some("crates-io".into()),
            offline: Some(true),
            ..ReleaseConfig::default()
        },
        ..CommonEcosystemConfig::default()
    };
    let adapter = FakeConfiguredAdapter::new(eid())
        .with_response(response)
        .with_common(common)
        .with_release_target(target);
    FakeProvider::new(eid()).with_adapter(adapter)
}

/// A committed repository with a single umbrella tag `v0.1.0`, per-crate tag
/// schemes, and a source edit in `core` (only) after the umbrella tag.
fn umbrella_repo() -> (TestWorkspace, AbsPath, Document) {
    let ws = TestWorkspace::new("release-umbrella-baseline");
    let scenario = GitScenario::init(ws.path()).expect("git init");
    scenario
        .commit_file(
            "toven.toml",
            concat!(
                "[project]\n",
                "name = \"workspace\"\n\n",
                "[ecosystems.rust]\n\n",
                "[ecosystems.rust.release]\n",
                "registry = \"crates-io\"\n",
                "offline = true\n\n",
                // The umbrella module cuts the shared `v{version}` tag; the
                // per-crate modules keep the default `rust/<crate>@` scheme,
                // which the umbrella tag never matches.
                "[modules.\"rust:suite\".release]\n",
                "umbrella = true\n",
                "tag_format = \"v{version}\"\n",
                "push = false\n",
            ),
            "config",
        )
        .expect("commit config");
    scenario
        .commit_file("crates/suite/src/lib.rs", "//! suite\n", "baseline suite")
        .expect("suite baseline");
    scenario
        .commit_file("crates/core/src/lib.rs", "pub fn a() {}\n", "baseline core")
        .expect("core baseline");
    scenario
        .commit_file("crates/util/src/lib.rs", "pub fn b() {}\n", "baseline util")
        .expect("util baseline");
    scenario
        .tag("v0.1.0", "release v0.1.0")
        .expect("tag the shared umbrella baseline");
    // Only `core` changes after the umbrella tag.
    scenario
        .commit_file(
            "crates/core/src/lib.rs",
            "pub fn a() -> u32 { 1 }\n",
            "a core change to release",
        )
        .expect("core change commit");

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
    PlanRequest::new("rel", "workspace", TaskIntent::resolve("release"), root)
}

#[test]
fn only_the_crate_changed_since_the_umbrella_tag_bumps() {
    let (ws, root, document) = umbrella_repo();
    let provider = umbrella_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = RskitGitVcs::open(ws.path()).expect("open reader");
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

    // `core` changed since the umbrella tag: it bumps by a real diff, off the
    // umbrella/registry anchor — never an initial release, and `From != To`.
    let core = plan
        .entries
        .iter()
        .find(|entry| entry.module.to_string() == "rust:core")
        .expect("core is in the plan");
    let planned = core.planned_version.as_ref().expect("core is bumped");
    assert_ne!(
        planned, &core.current_version,
        "the changed crate must advance (From != To): {core:?}"
    );
    assert_eq!(
        planned,
        &Version::new(0, 1, 1),
        "a single source change patches the crate off the 0.1.0 anchor: {core:?}"
    );
    assert_ne!(
        core.reason,
        BumpReason::InitialRelease,
        "a crate with an umbrella/registry anchor is not a first release: {core:?}"
    );
    let baseline = core.baseline.as_ref().expect("core baseline recorded");
    assert!(
        !baseline.is_initial(),
        "the umbrella/registry anchor is a released baseline: {baseline:?}"
    );
    assert_eq!(
        baseline.version,
        Some(Version::new(0, 1, 0)),
        "the baseline version is the umbrella/registry anchor: {baseline:?}"
    );
    assert_eq!(
        baseline.tag.as_deref(),
        Some("v0.1.0"),
        "the diff anchor is the shared umbrella tag: {baseline:?}"
    );

    // `util` did not change since the umbrella tag: it is not treated as a first
    // release and never joins the plan (no `From == To` entry).
    assert!(
        plan.entries
            .iter()
            .all(|entry| entry.module.to_string() != "rust:util"),
        "an unchanged crate must not bump: {:?}",
        plan.entries
    );
}

/// A committed repository that misconfigures two `umbrella = true` modules in
/// the same member, leaving the `baseline`/`tag_mode` knobs unset so the
/// umbrella anchor is inferred from umbrella presence (the path that bypasses
/// the explicit-selector plan validation).
fn two_umbrella_repo() -> (TestWorkspace, AbsPath, Document) {
    let ws = TestWorkspace::new("release-two-umbrella");
    let scenario = GitScenario::init(ws.path()).expect("git init");
    scenario
        .commit_file(
            "toven.toml",
            concat!(
                "[project]\n",
                "name = \"workspace\"\n\n",
                "[ecosystems.rust]\n\n",
                "[ecosystems.rust.release]\n",
                "registry = \"crates-io\"\n",
                "offline = true\n\n",
                "[modules.\"rust:suite\".release]\n",
                "umbrella = true\n",
                "tag_format = \"v{version}\"\n",
                "push = false\n\n",
                // A second umbrella module in the same member: the umbrella tag
                // is now ambiguous.
                "[modules.\"rust:core\".release]\n",
                "umbrella = true\n",
                "tag_format = \"v{version}\"\n",
                "push = false\n",
            ),
            "config",
        )
        .expect("commit config");
    scenario
        .commit_file("crates/suite/src/lib.rs", "//! suite\n", "baseline suite")
        .expect("suite baseline");
    scenario
        .commit_file("crates/core/src/lib.rs", "pub fn a() {}\n", "baseline core")
        .expect("core baseline");
    scenario
        .commit_file("crates/util/src/lib.rs", "pub fn b() {}\n", "baseline util")
        .expect("util baseline");
    scenario
        .tag("v0.1.0", "release v0.1.0")
        .expect("tag the shared umbrella baseline");
    scenario
        .commit_file(
            "crates/core/src/lib.rs",
            "pub fn a() -> u32 { 1 }\n",
            "a core change to release",
        )
        .expect("core change commit");

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

#[test]
fn two_umbrella_modules_fail_closed_on_the_inferred_baseline_path() {
    // With `baseline`/`tag_mode` unset, the umbrella anchor is inferred from
    // umbrella presence, so the explicit-selector plan validation never fires.
    // Resolving "the" umbrella scheme must still fail closed rather than pick an
    // arbitrary umbrella module.
    let (ws, root, document) = two_umbrella_repo();
    let provider = umbrella_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = RskitGitVcs::open(ws.path()).expect("open reader");
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("HEAD"));
    let mut reporter = RecordingReporter::new();

    let error = release_plan(
        &request(root),
        &document,
        &providers,
        &readers,
        &BumpOverrides::new(),
        &mut reporter,
    )
    .expect_err("two umbrella modules in one member is a fail-closed misconfiguration");

    let message = error.to_string();
    assert!(
        message.contains("more than one umbrella module"),
        "the error names the ambiguous umbrella misconfiguration: {message}"
    );
}

/// A committed repository in explicit `umbrella` tag mode where only a
/// non-umbrella crate changes, so the umbrella module is not bumped and its
/// entry never appears in the plan.
fn umbrella_mode_repo() -> (TestWorkspace, AbsPath, Document) {
    let ws = TestWorkspace::new("release-umbrella-mode");
    let scenario = GitScenario::init(ws.path()).expect("git init");
    scenario
        .commit_file(
            "toven.toml",
            concat!(
                "[project]\n",
                "name = \"workspace\"\n\n",
                "[ecosystems.rust]\n\n",
                "[ecosystems.rust.release]\n",
                "registry = \"crates-io\"\n",
                "offline = true\n\n",
                // The umbrella module governs the shared umbrella tag; in
                // `umbrella` mode it is the only tag the train cuts.
                "[modules.\"rust:suite\".release]\n",
                "umbrella = true\n",
                "tag_mode = \"umbrella\"\n",
                "tag_format = \"v{version}\"\n",
                "push = false\n",
            ),
            "config",
        )
        .expect("commit config");
    scenario
        .commit_file("crates/suite/src/lib.rs", "//! suite\n", "baseline suite")
        .expect("suite baseline");
    scenario
        .commit_file("crates/core/src/lib.rs", "pub fn a() {}\n", "baseline core")
        .expect("core baseline");
    scenario
        .commit_file("crates/util/src/lib.rs", "pub fn b() {}\n", "baseline util")
        .expect("util baseline");
    scenario
        .tag("v0.1.0", "release v0.1.0")
        .expect("tag the shared umbrella baseline");
    scenario
        .commit_file(
            "crates/core/src/lib.rs",
            "pub fn a() -> u32 { 1 }\n",
            "a core change to release",
        )
        .expect("core change commit");

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

#[test]
fn umbrella_mode_without_the_umbrella_bumping_fails_closed() {
    // In `umbrella` tag mode the shared `v{version}` tag is cut only by the
    // umbrella module. When only a non-umbrella crate changes, the umbrella
    // module is not bumped and has no entry, so the train would publish with no
    // tag at all. Planning must refuse rather than anchor the release on nothing.
    let (ws, root, document) = umbrella_mode_repo();
    let provider = umbrella_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = RskitGitVcs::open(ws.path()).expect("open reader");
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("HEAD"));
    let mut reporter = RecordingReporter::new();

    let error = release_plan(
        &request(root),
        &document,
        &providers,
        &readers,
        &BumpOverrides::new(),
        &mut reporter,
    )
    .expect_err("umbrella mode must not release a train without cutting the umbrella tag");

    let message = error.to_string();
    assert!(
        message.contains("umbrella tag would never be cut") && message.contains("publish untagged"),
        "the error explains the uncut umbrella tag and untagged publish: {message}"
    );
}

#[test]
fn a_registry_outage_downgrades_to_the_umbrella_tag_anchor() {
    // the umbrella tag still anchors the diff: change detection must complete on
    // the umbrella-tag anchor rather than aborting the whole run.
    let (ws, root, document) = umbrella_repo();
    let provider = umbrella_provider_with(
        FakeReleaseTarget::new()
            .with_declared_version(Version::new(0, 1, 0))
            .with_published_read_failure("registry offline"),
    );
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = RskitGitVcs::open(ws.path()).expect("open reader");
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
    .expect("a registry outage must not abort change detection");

    let core = plan
        .entries
        .iter()
        .find(|entry| entry.module.to_string() == "rust:core")
        .expect("core is still in the plan");
    assert_ne!(
        core.reason,
        BumpReason::InitialRelease,
        "the umbrella-tag anchor keeps a changed crate off the initial-release path: {core:?}"
    );
    let baseline = core.baseline.as_ref().expect("core baseline recorded");
    assert_eq!(
        baseline.version,
        Some(Version::new(0, 1, 0)),
        "the downgraded baseline anchors on the umbrella tag version: {baseline:?}"
    );
    assert_eq!(
        core.planned_version.as_ref(),
        Some(&Version::new(0, 1, 1)),
        "the changed crate still advances off the umbrella-tag anchor: {core:?}"
    );
}
