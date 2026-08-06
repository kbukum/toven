//! Mixed-repository release flow: registry libraries and a self-hosted binary
//! app release together in one shared-`v{version}` train.
//!
//! A single repository can carry modules published as registry **libraries**
//! and one module released as a self-hosted **binary app**. They release under
//! one shared tag: the libraries publish to the registry and contribute release
//! notes, while a per-module override makes the binary tag-only and attaches the
//! signed archive/checksum assets to exactly that module. Every module rendering
//! the shared `v{version}` tag collapses into one hosted Release whose notes and
//! assets are the union of the per-module contributions.
//!
//! This locks the composition end to end through the engine's real settings
//! resolution, plan, and rehearsal projections.

use std::collections::BTreeSet;

use toven_engine_core::config::{CanonicalRegistry, Document, load};
use toven_engine_core::federation::MemberVcsReaders;
use toven_engine_core::plan::{PlanRequest, Selection};
use toven_engine::release::{BumpOverrides, PublishDecision, release_rehearse, release_status};
use toven_engine_core::vcs::RskitGitVcs;
use toven_model::{AbsPath, EcosystemId, Module, ModuleRef, RepoPath};
use toven_ports::{
    BaselineSpec, CommonEcosystemConfig, DiscoverResponse, HostConfig, Provider, ReleaseConfig,
    TaskIntent,
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

/// A rust provider exposing a registry library (`corelib`) and a binary app
/// (`app`), both in the single (degenerate) member repo. The ecosystem declares
/// a crates.io registry and a `github` forge; the app's per-module override (in
/// the loaded `toven.toml`) narrows it to tag-only and owns the release assets.
fn mixed_provider() -> FakeProvider {
    let mut response = DiscoverResponse::new(eid());
    response.modules.push(Module::new(
        mref("corelib"),
        RepoPath::new("crates/corelib").expect("corelib path"),
    ));
    response.modules.push(Module::new(
        mref("app"),
        RepoPath::new("crates/app").expect("app path"),
    ));
    let common = CommonEcosystemConfig {
        release: ReleaseConfig {
            // The declared workspace version is strictly ahead of the last
            // shared release tag, so the `manifest` strategy cuts it.
            strategy: Some("manifest".into()),
            tag_format: Some("v{version}".into()),
            registry: Some("crates-io".into()),
            offline: Some(true),
            host: Some(HostConfig {
                forge: Some("github".into()),
                ..HostConfig::default()
            }),
            ..ReleaseConfig::default()
        },
        ..CommonEcosystemConfig::default()
    };
    // The workspace shares one version: both modules declare `0.1.1`, one tag
    // above the `v0.1.0` baseline.
    let target = FakeReleaseTarget::new()
        .with_declared_version(rskit_version::semver::Version::new(0, 1, 1));
    let adapter = FakeConfiguredAdapter::new(eid())
        .with_response(response)
        .with_common(common)
        .with_release_target(target);
    FakeProvider::new(eid()).with_adapter(adapter)
}

/// A committed repository whose `toven.toml` releases `corelib` as a registry
/// library and `app` as a tag-only binary owning the hosted assets, sharing one
/// `v{version}` tag; a prior `v0.1.0` tag anchors the follow-up release.
fn mixed_repo() -> (TestWorkspace, AbsPath, Document) {
    let ws = TestWorkspace::new("release-mixed-repo");
    let scenario = GitScenario::init(ws.path()).expect("git init");
    scenario
        .commit_file(
            "toven.toml",
            concat!(
                "[project]\n",
                "name = \"mixed\"\n\n",
                "[ecosystems.rust]\n\n",
                "[ecosystems.rust.release]\n",
                "strategy = \"manifest\"\n",
                "tag_format = \"v{version}\"\n",
                "registry = \"crates-io\"\n",
                "offline = true\n\n",
                "[ecosystems.rust.release.host]\n",
                "forge = \"github\"\n\n",
                "[modules.\"rust:app\".release]\n",
                "publish = false\n\n",
                "[modules.\"rust:app\".release.host]\n",
                "forge = \"github\"\n",
                "assets = [\n",
                "  \"dist/mixed-app-x86_64-unknown-linux-gnu.tar.gz\",\n",
                "  \"dist/SHA256SUMS\",\n",
                "]\n",
            ),
            "config",
        )
        .expect("commit config");
    scenario
        .commit_file(
            "crates/corelib/src/lib.rs",
            "pub fn a() {}\n",
            "baseline source",
        )
        .expect("baseline commit");
    scenario
        .commit_file("crates/app/src/main.rs", "fn main() {}\n", "baseline app")
        .expect("baseline app commit");
    scenario
        .tag("v0.1.0", "release v0.1.0")
        .expect("tag the shared release baseline");
    scenario
        .commit_file(
            "crates/corelib/src/lib.rs",
            "pub fn a() -> u32 { 1 }\n",
            "a library change to release",
        )
        .expect("library change commit");
    scenario
        .commit_file(
            "crates/app/src/main.rs",
            "fn main() { println!(\"1\"); }\n",
            "an app change to release",
        )
        .expect("app change commit");

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
    PlanRequest::new("rel", "mixed", TaskIntent::resolve("release"), root)
        .with_selection(Selection::All)
}

#[test]
fn libraries_and_a_binary_app_release_in_one_shared_tag_train() {
    let (ws, root, document) = mixed_repo();
    let provider = mixed_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = RskitGitVcs::open(ws.path()).expect("open reader");
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("v0.1.0"));
    let mut reporter = RecordingReporter::new();

    let rehearsal = release_rehearse(
        &request(root),
        &document,
        &providers,
        &readers,
        &BumpOverrides::new(),
        &mut reporter,
        false,
    )
    .expect("rehearse the mixed release");

    // Per-module verdicts: the library publishes to the registry; the binary is
    // tag-only. Publication policy and host participation stay orthogonal.
    let corelib = rehearsal
        .verdicts
        .iter()
        .find(|verdict| verdict.module.to_string() == "rust:corelib")
        .expect("corelib verdict present");
    let app = rehearsal
        .verdicts
        .iter()
        .find(|verdict| verdict.module.to_string() == "rust:app")
        .expect("app verdict present");
    assert_ne!(
        corelib.decision,
        PublishDecision::TagOnly,
        "the registry library publishes to the registry: {corelib:?}"
    );
    assert_eq!(
        app.decision,
        PublishDecision::TagOnly,
        "the binary app is tag-only: {app:?}"
    );

    // One shared tag is one hosted Release carrying exactly the binary module's
    // assets — the library contributed notes, not archives.
    assert_eq!(
        rehearsal.hosted.len(),
        1,
        "one shared `v{{version}}` tag is one hosted Release: {:?}",
        rehearsal.hosted
    );
    let hosted = &rehearsal.hosted[0];
    assert_eq!(hosted.forge, "github");
    assert_eq!(hosted.tag, "v0.1.1");
    assert_eq!(
        hosted.assets,
        vec![
            "dist/mixed-app-x86_64-unknown-linux-gnu.tar.gz".to_string(),
            "dist/SHA256SUMS".to_string(),
        ],
        "hosted assets are scoped to the binary-producing module"
    );
}

#[test]
fn status_shows_per_module_policy_and_host_participation() {
    let (ws, root, document) = mixed_repo();
    let provider = mixed_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = RskitGitVcs::open(ws.path()).expect("open reader");
    let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("v0.1.0"));
    let mut reporter = RecordingReporter::new();

    let status = release_status(
        &request(root),
        &document,
        &providers,
        &readers,
        &mut reporter,
    )
    .expect("status of the mixed repo");

    let corelib = status
        .modules
        .iter()
        .find(|module| module.module.to_string() == "rust:corelib")
        .expect("corelib status present");
    let app = status
        .modules
        .iter()
        .find(|module| module.module.to_string() == "rust:app")
        .expect("app status present");

    // The library publishes to the registry; the binary is tag-only. Both
    // participate in the host phase (they feed the one shared Release).
    assert_eq!(
        corelib.publication,
        toven_ports::PublicationPolicy::Registry {
            registry: "crates-io".into()
        }
    );
    assert_eq!(app.publication, toven_ports::PublicationPolicy::TagOnly);
    assert_eq!(corelib.host_forge.as_deref(), Some("github"));
    assert_eq!(app.host_forge.as_deref(), Some("github"));
}
