//! `toven release` command tests.

use std::collections::BTreeSet;

use rskit_cli::ExitCode;
use rskit_util::time::FixedClock;
use toven_core::config::{CanonicalRegistry, Document, load};
use toven_core::federation::MemberVcsReaders;
use toven_core::federation::baseline::MemberVcsReader;
use toven_core::federation::member_repo::{MemberReleaseRepo, MemberReleaseRepos};
use toven_core::plan::PlanRequest;
use toven_model::{
    AbsPath, EcosystemId, Event, Module, ModuleRef, OutcomeSummary, RepoPath, ToolchainTag,
    Workspace, WorkspaceId,
};
use toven_ports::{DiscoverResponse, Provider, TaskIntent};
use toven_release::{BumpOptions, BumpOverrides};
use toven_testkit::workspace::workspace;
use toven_testkit::{
    FakeConfiguredAdapter, FakeProvider, FakeReleaseTarget, FakeVcsReader, FakeVcsWriter,
    RecordingHookRunner, RecordingReporter, TestWorkspace,
};

use super::lifecycle::require_release_confirmation;
use super::lifecycle::stream_bump;
use super::summary::release_exit;

#[test]
fn empty_release_plan_summary_states_the_up_to_date_fact() {
    use toven_release::{BumpPolicy, ReleasePlan};

    use super::summary::plan_summary_line;

    // An up-to-date workspace resolves an empty plan: the streamed decision
    // events emit nothing, so the closing summary must state the up-to-date
    // fact outright rather than leave a silent stream.
    let empty = ReleasePlan::new(BumpPolicy::SemverCascade, Vec::new());
    assert_eq!(
        plan_summary_line(&empty),
        "release: nothing to release — all modules up to date",
    );
}

#[test]
fn run_summary_line_phrases_by_the_landed_side_effect() {
    use super::summary::run_summary_line;

    assert_eq!(run_summary_line("bump", 2), "release: 2 staged");
    assert_eq!(run_summary_line("tag", 3), "release: 3 tagged");
    assert_eq!(run_summary_line("publish", 1), "release: 1 published");
}

#[test]
fn real_release_requires_explicit_confirmation() {
    let error = require_release_confirmation(false).expect_err("confirmation is required");
    assert!(error.to_string().contains("release.confirmation"));
    assert!(error.to_string().contains("--yes"));
}

#[test]
fn explicit_confirmation_allows_real_release_to_continue() {
    require_release_confirmation(true).expect("confirmation permits release");
}

#[test]
fn release_exit_is_derived_from_the_item_summary() {
    // The exit owner is the item-based summary, never a hardcoded success:
    // an all-succeeded run exits zero, any failed item exits non-zero.
    let clean = OutcomeSummary {
        processed: 3,
        succeeded: 3,
        failed: 0,
        skipped: 0,
    };
    assert_eq!(release_exit(&clean), ExitCode::Success);

    let failed = OutcomeSummary {
        processed: 3,
        succeeded: 2,
        failed: 1,
        skipped: 0,
    };
    assert_eq!(release_exit(&failed), ExitCode::Failure);
}

/// A single-`core`-module rust provider rooted at the repo, releasable
/// through the default fake release target (declares a version, rewrites a
/// manifest — enough for a `bump` to stage a mutation).
fn core_provider() -> FakeProvider {
    let eid = EcosystemId::new("rust").expect("ecosystem id");
    let mut response = DiscoverResponse::new(eid.clone());
    response.workspaces.push(Workspace::new(
        WorkspaceId::new("rust").expect("workspace id"),
        RepoPath::new(".").expect("root"),
        ToolchainTag::new("cargo"),
    ));
    let mut module = Module::new(
        ModuleRef::new(eid.clone(), "core").expect("module ref"),
        RepoPath::new(".").expect("root"),
    );
    module.workspace = Some(WorkspaceId::new("rust").expect("workspace id"));
    response.modules.push(module);
    let adapter = FakeConfiguredAdapter::new(eid.clone())
        .with_response(response)
        .with_release_target(FakeReleaseTarget::new());
    FakeProvider::new(eid).with_adapter(adapter)
}

/// Load a single-repo project whose `core` module stages a version bump.
fn single_module_project() -> (TestWorkspace, AbsPath, Document) {
    let ws = workspace("release-cli-bump");
    let body = "[project]\nname = \"solo\"\n\n[ecosystems.rust]\nmanifests = [\"Cargo.toml\"]\n\n[modules.\"rust:core\".release]\npush = false\ncommit_message = \"release {module} {version}\"\n";
    let path = ws
        .write_file("toven.toml", body.as_bytes())
        .expect("write project");
    let root = AbsPath::new(ws.path().to_path_buf()).expect("absolute root");
    let document = load(&path, &BTreeSet::new(), &CanonicalRegistry::model())
        .expect("project loads")
        .document;
    (ws, root, document)
}

#[test]
fn stream_bump_feeds_the_live_sink_decision_then_commit() {
    // The CLI seam must route the engine's per-module events into the live
    // reporter — the decision (resolved) up front, then the commit (staged)
    // once the mutation lands — and derive the exit from the run summary.
    let (ws, root, document) = single_module_project();
    let provider = core_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let reader = FakeVcsReader::new();
    let writer = FakeVcsWriter::new().with_commit_oid("unused");
    let readers = MemberVcsReaders::new(vec![MemberVcsReader::new(None, ".", None, &reader)]);
    let repos = MemberReleaseRepos::new(vec![MemberReleaseRepo::new(
        None,
        ws.path().to_path_buf(),
        &reader,
        &writer,
    )]);
    let request = PlanRequest::new("bump-1", "solo", TaskIntent::resolve("release"), root);
    let clock = FixedClock::new(1_718_409_600, 0);
    let hooks = RecordingHookRunner::new();
    let mut sink = RecordingReporter::new();

    let exit = stream_bump(
        &request,
        &document,
        &providers,
        &readers,
        &repos,
        &BumpOverrides::new(),
        &mut sink,
        &clock,
        &hooks,
        BumpOptions::default(),
        crate::flags::OutputKind::Jsonl,
        crate::flags::ColorWhen::Never,
    )
    .expect("bump streams");

    // The summary-derived exit for a clean bump is success.
    assert_eq!(exit, ExitCode::Success);

    let resolved = sink
        .events()
        .iter()
        .position(|event| matches!(event, Event::ModuleReleaseResolved { .. }))
        .expect("a decision event is streamed");
    let staged = sink
        .events()
        .iter()
        .position(|event| matches!(event, Event::ModuleReleaseStaged { .. }))
        .expect("a commit event is streamed");
    assert!(
        resolved < staged,
        "the decision must precede the commit: {:?}",
        sink.events()
    );
}

#[test]
fn status_line_renders_unreleased_when_version_is_absent() {
    use super::inspect::status_line;
    use toven_ports::PublicationPolicy;
    use toven_release::ReleaseModuleStatus;

    let status = ReleaseModuleStatus {
        module: ModuleRef::new(EcosystemId::new("go").unwrap(), "core")
            .unwrap()
            .into(),
        publication: PublicationPolicy::TagOnly,
        declared_version: None,
        latest_tag: None,
        host_forge: None,
        published_versions: Vec::new(),
        is_published: false,
        entrypoint: toven_model::Entrypoint::Toven,
        maintainer_tag_present: None,
    };
    assert_eq!(
        status_line(&status),
        "go:core  tag-only  declared unreleased  tag -  hosted -  toven  unpublished"
    );
}

#[test]
fn status_record_omits_declared_version_when_absent_in_jsonl() {
    use super::inspect::status_record;
    use toven_ports::PublicationPolicy;
    use toven_release::ReleaseModuleStatus;

    let status = ReleaseModuleStatus {
        module: ModuleRef::new(EcosystemId::new("go").unwrap(), "core")
            .unwrap()
            .into(),
        publication: PublicationPolicy::TagOnly,
        declared_version: None,
        latest_tag: None,
        host_forge: None,
        published_versions: Vec::new(),
        is_published: false,
        entrypoint: toven_model::Entrypoint::Toven,
        maintainer_tag_present: None,
    };
    let json = serde_json::to_string(&status_record(&status)).expect("serializes to json");
    assert!(
        !json.contains("declared_version"),
        "absent declared_version should be omitted from jsonl record: {json}"
    );
    assert!(json.contains("\"module\":\"go:core\""));
    assert!(json.contains("\"publication\":\"tag-only\""));
}

#[test]
fn status_line_and_record_render_concrete_declared_version() {
    use super::inspect::status_line;
    use super::inspect::status_record;
    use rskit_version::semver::Version;
    use toven_ports::PublicationPolicy;
    use toven_release::ReleaseModuleStatus;

    let status = ReleaseModuleStatus {
        module: ModuleRef::new(EcosystemId::new("rust").unwrap(), "core")
            .unwrap()
            .into(),
        publication: PublicationPolicy::Registry {
            registry: "crates-io".into(),
        },
        declared_version: Some(Version::new(0, 2, 0)),
        latest_tag: Some("rust/core@0.1.0".into()),
        host_forge: Some("github".into()),
        published_versions: vec![Version::new(0, 1, 0)],
        is_published: false,
        entrypoint: toven_model::Entrypoint::Toven,
        maintainer_tag_present: None,
    };
    assert_eq!(
        status_line(&status),
        "rust:core  registry (crates-io)  declared 0.2.0  tag rust/core@0.1.0  hosted github  toven  unpublished"
    );
    let json = serde_json::to_string(&status_record(&status)).expect("serializes to json");
    assert!(json.contains("\"declared_version\":\"0.2.0\""));
}
