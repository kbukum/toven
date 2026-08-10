//! Watch-loop tests: the [`WatchSession`] rerun loop over scripted change
//! batches, exercised end-to-end against fake adapters, VCS, prober, and cache.

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use common::eid;
use toven_core::config::{Document, ProjectConfig, TovenConfig};
use toven_core::federation::MemberVcsReaders;
use toven_core::federation::baseline::MemberVcsReader;
use toven_core::plan::{PlanRequest, Selection};
use toven_engine::apply::ApplyOptions;
use toven_engine::output::UnitOutputChannel;
use toven_engine::watch::WatchSession;
use toven_model::{
    AbsPath, DepKind, Edge, Event, MemberId, Module, ModuleRef, RepoPath, ToolchainTag, Workspace,
    WorkspaceId,
};
use toven_ports::{CommandRunner, DiscoverResponse, FanOut, Provider, Task, TaskIntent};
use toven_testkit::{
    CountingToolchainProber, FakeCacheStore, FakeCommandRunner, FakeConfiguredAdapter,
    FakeProvider, FakeSourceDigest, FakeVcsReader, RecordingCacheWriter, RecordingRawOutputSink,
    RecordingReporter, ScriptedWatchSource,
};

fn mref(ecosystem: &str, name: &str) -> ModuleRef {
    ModuleRef::new(eid(ecosystem), name).expect("valid module ref")
}

fn wsid(id: &str) -> WorkspaceId {
    WorkspaceId::new(id).expect("valid workspace id")
}

fn module(name: &str, root: &str) -> Module {
    let mut module = Module::new(mref("rust", name), RepoPath::new(root).expect("root"));
    module.workspace = Some(wsid("rust"));
    module
}

/// A two-module rust workspace (`app` depends on `errors`) with one Test task.
fn rust_provider() -> FakeProvider {
    let mut response = DiscoverResponse::new(eid("rust"));
    response.workspaces.push(Workspace::new(
        wsid("rust"),
        RepoPath::new(".").expect("root"),
        ToolchainTag::new("cargo"),
    ));
    response.modules.push(module("errors", "crates/errors"));
    response.modules.push(module("app", "crates/app"));
    response.edges.push(Edge::new(
        mref("rust", "app"),
        mref("rust", "errors"),
        DepKind::Normal,
    ));

    let task = Task::new(
        "test",
        vec!["cargo".to_string(), "test".to_string()],
        FanOut::PerModule,
    );
    let adapter = FakeConfiguredAdapter::new(eid("rust"))
        .with_response(response)
        .with_tasks(vec![task]);
    FakeProvider::new(eid("rust")).with_adapter(adapter)
}

fn document() -> Document {
    Document {
        project: ProjectConfig {
            name: "toven".to_string(),
            root: ".".to_string(),
            base_ref: None,
        },
        toven: TovenConfig::default(),
        groups: BTreeMap::new(),
        overlays: Vec::new(),
        ecosystems: BTreeMap::from([(eid("rust"), serde_json::json!({}))]),
        modules: std::collections::BTreeMap::new(),
        members: Vec::new(),
        hooks: std::collections::BTreeMap::new(),
    }
}

fn request() -> PlanRequest {
    PlanRequest::new(
        "run-watch",
        "toven",
        TaskIntent::resolve("test"),
        AbsPath::new("/repo").expect("absolute"),
    )
    .with_selection(Selection::All)
}

/// Drive a [`WatchSession`] over `batches` (absolute paths) and return the
/// captured events plus the recorded watch calls.
fn drive(
    batches: Vec<Vec<PathBuf>>,
    vcs: &FakeVcsReader,
) -> (Vec<Event>, Vec<toven_testkit::WatchCall>) {
    let batches = batches
        .into_iter()
        .map(toven_ports::ChangeBatch::new)
        .collect();
    drive_batches(batches, vcs)
}

/// Like [`drive`], but replays [`ChangeBatch`]es verbatim so tests can script a
/// rescan signal.
fn drive_batches(
    batches: Vec<toven_ports::ChangeBatch>,
    vcs: &FakeVcsReader,
) -> (Vec<Event>, Vec<toven_testkit::WatchCall>) {
    let readers = MemberVcsReaders::single(vcs, toven_ports::BaselineSpec::explicit("main"));
    drive_batches_with_readers(batches, &readers)
}

/// Drive the watch loop over verbatim batches against an explicit reader view,
/// so tests can exercise federated/member-scoped reader arrangements.
fn drive_batches_with_readers(
    batches: Vec<toven_ports::ChangeBatch>,
    readers: &MemberVcsReaders<'_>,
) -> (Vec<Event>, Vec<toven_testkit::WatchCall>) {
    let (result, events, calls) = drive_request_with_readers(request(), batches, readers);
    result.expect("watch loop");
    (events, calls)
}

fn drive_request_with_readers(
    request: PlanRequest,
    batches: Vec<toven_ports::ChangeBatch>,
    readers: &MemberVcsReaders<'_>,
) -> (
    rskit_errors::AppResult<toven_model::RunStats>,
    Vec<Event>,
    Vec<toven_testkit::WatchCall>,
) {
    let provider = rust_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let digest = FakeSourceDigest::new();
    let prober = CountingToolchainProber::new();
    let cache_store = FakeCacheStore::new();
    let cache_writer = RecordingCacheWriter::new();
    let runner: Arc<dyn CommandRunner> = Arc::new(FakeCommandRunner::new());
    let mut output = UnitOutputChannel::new(RecordingRawOutputSink::new());
    let mut reporter = RecordingReporter::new();
    let watch = ScriptedWatchSource::from_batches(batches);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .enable_io()
        .build()
        .expect("runtime");
    let document = document();
    let result = runtime.block_on(
        WatchSession {
            request,
            document: &document,
            providers: &providers,
            readers,
            digest: &digest,
            prober: &prober,
            cache_store: &cache_store,
            cache_writer: &cache_writer,
            runner,
            apply_options: ApplyOptions::default(),
            watch: &watch,
            debounce: Duration::from_millis(200),
            reporter: &mut reporter,
            output: &mut output,
            cancel: tokio_util::sync::CancellationToken::new(),
        }
        .run(),
    );

    (result, reporter.events().to_vec(), watch.calls())
}

#[test]
fn baseline_run_then_watch_started_and_stopped_bracket_the_loop() {
    let (events, calls) = drive(Vec::new(), &FakeVcsReader::new());

    assert_eq!(count_started(&events), 1, "one WatchStarted");
    assert_eq!(count_stopped(&events), 1, "one WatchStopped");
    // With no batches, only the baseline iteration runs.
    assert_eq!(count_run_started(&events), 1, "only the baseline run");
    assert_eq!(count_triggered(&events), 0, "no change triggered a rerun");

    assert_eq!(calls.len(), 1, "one watch subscription");
    assert_eq!(calls[0].roots, vec![PathBuf::from("/repo")]);
    assert_eq!(calls[0].debounce, Duration::from_millis(200));

    // WatchStarted precedes the baseline run; WatchStopped is last.
    assert!(matches!(events.first(), Some(Event::WatchStarted { .. })));
    assert!(matches!(events.last(), Some(Event::WatchStopped)));
}

#[test]
fn failed_baseline_plan_does_not_emit_a_run_header() {
    let vcs = FakeVcsReader::new();
    let readers = MemberVcsReaders::single(&vcs, toven_ports::BaselineSpec::explicit("main"));
    let invalid = PlanRequest::new(
        "run-watch",
        "toven",
        TaskIntent::resolve("unknown"),
        AbsPath::new("/repo").expect("absolute"),
    )
    .with_selection(Selection::All);

    let (result, events, _calls) = drive_request_with_readers(invalid, Vec::new(), &readers);

    assert!(result.is_err(), "an unknown watched task must fail PLAN");
    assert!(matches!(events.first(), Some(Event::WatchStarted { .. })));
    assert_eq!(
        count_run_started(&events),
        0,
        "failed PLAN never starts a run"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::PhaseStarted { .. })),
        "failed PLAN lifecycle framing is discarded"
    );
}

#[test]
fn a_changed_source_path_triggers_one_rerun() {
    let batches = vec![vec![PathBuf::from("/repo/crates/app/src/lib.rs")]];
    let (events, _calls) = drive(batches, &FakeVcsReader::new());

    assert_eq!(count_triggered(&events), 1, "the change triggered a rerun");
    // Baseline + the one change-driven rerun.
    assert_eq!(count_run_started(&events), 2, "baseline plus one rerun");

    let triggered = events.iter().find_map(|event| match event {
        Event::WatchTriggered { paths } => Some(paths.clone()),
        _ => None,
    });
    assert_eq!(
        triggered,
        Some(vec!["crates/app/src/lib.rs".to_string()]),
        "the workspace-relative changed path is reported"
    );
}

#[test]
fn an_empty_batch_does_not_trigger_a_rerun() {
    let batches = vec![vec![]];
    let (events, _calls) = drive(batches, &FakeVcsReader::new());

    assert_eq!(count_triggered(&events), 0, "an empty batch is skipped");
    assert_eq!(count_run_started(&events), 1, "only the baseline run");
}

#[test]
fn an_all_ignored_batch_does_not_trigger_a_rerun() {
    let vcs = FakeVcsReader::new().with_ignored(vec![PathBuf::from("crates/app/target/debug/app")]);
    let batches = vec![vec![PathBuf::from("/repo/crates/app/target/debug/app")]];
    let (events, _calls) = drive(batches, &vcs);

    assert_eq!(
        count_triggered(&events),
        0,
        "ignored paths are filtered out"
    );
    assert_eq!(count_run_started(&events), 1, "only the baseline run");
}

#[test]
fn a_member_scoped_readers_ignore_rules_are_not_applied_to_root_paths() {
    // In a federated setup, entries() holds member-scoped readers (member() is
    // Some) whose ignore rules apply to their own repo root, not the umbrella root.
    // is_ignored must consult only the degenerate/root entry, so a member reader
    // that would "ignore" this path must NOT suppress the rerun.
    let vcs = FakeVcsReader::new().with_ignored(vec![PathBuf::from("crates/app/src/lib.rs")]);
    let readers = MemberVcsReaders::new(vec![MemberVcsReader::new(
        Some(MemberId::new("app").expect("valid member id")),
        PathBuf::new(),
        Some(toven_ports::BaselineSpec::explicit("main")),
        &vcs,
    )]);
    let batches = vec![toven_ports::ChangeBatch::new(vec![PathBuf::from(
        "/repo/crates/app/src/lib.rs",
    )])];
    let (events, _calls) = drive_batches_with_readers(batches, &readers);

    assert_eq!(
        count_triggered(&events),
        1,
        "a member reader's ignore rules must not suppress a root-relative change"
    );
}

#[test]
fn a_path_outside_the_workspace_root_is_ignored() {
    let batches = vec![vec![PathBuf::from("/elsewhere/file.rs")]];
    let (events, _calls) = drive(batches, &FakeVcsReader::new());

    assert_eq!(
        count_triggered(&events),
        0,
        "paths outside the root are dropped"
    );
    assert_eq!(count_run_started(&events), 1, "only the baseline run");
}

#[test]
fn a_rescan_batch_reruns_the_baseline_scope() {
    // A rescan-only batch (overflow, no surviving paths) must still drive a full
    // rerun of the caller's baseline selection, not be skipped as "empty".
    let batches = vec![toven_ports::ChangeBatch::new(Vec::new()).with_rescan(true)];
    let (events, _calls) = drive_batches(batches, &FakeVcsReader::new());

    assert_eq!(count_rescan(&events), 1, "the rescan drove a rerun");
    assert_eq!(count_triggered(&events), 0, "no path-driven trigger");
    // Baseline + the rescan-driven rerun.
    assert_eq!(
        count_run_started(&events),
        2,
        "baseline plus one rescan rerun"
    );
}

#[test]
fn a_rescan_batch_ignores_its_partial_paths() {
    // Even with surviving paths, a rescan re-evaluates the whole scope rather than
    // trusting the (possibly incomplete) path list, so no WatchTriggered.
    let batch = toven_ports::ChangeBatch::new(vec![PathBuf::from("/repo/crates/app/src/lib.rs")])
        .with_rescan(true);
    let (events, _calls) = drive_batches(vec![batch], &FakeVcsReader::new());

    assert_eq!(count_rescan(&events), 1, "the rescan drove a rerun");
    assert_eq!(count_triggered(&events), 0, "partial paths are not trusted");
    assert_eq!(
        count_run_started(&events),
        2,
        "baseline plus one rescan rerun"
    );
}

fn count_started(events: &[Event]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, Event::WatchStarted { .. }))
        .count()
}

fn count_stopped(events: &[Event]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, Event::WatchStopped))
        .count()
}

fn count_triggered(events: &[Event]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, Event::WatchTriggered { .. }))
        .count()
}

fn count_rescan(events: &[Event]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, Event::WatchRescan))
        .count()
}

fn count_run_started(events: &[Event]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, Event::RunStarted { .. }))
        .count()
}
