//! End-to-end PLAN pipeline tests over fake adapters, VCS, prober, and cache.

mod common;

use std::collections::BTreeMap;

use common::{CountingProber, RecordingCache, SetCache, StubDigest, eid};
use toven_engine::config::{Document, ProjectConfig, TovenConfig};
use toven_engine::plan::{CacheMode, NullCache, PlanHost, PlanRequest, Selection, plan};
use toven_model::{
    AbsPath, CacheVerdict, DepKind, Edge, Event, Module, ModuleRef, Phase, Plan, RepoPath,
    ToolchainTag, Workspace, WorkspaceId,
};
use toven_ports::{DiscoverResponse, FanOut, Provider, Task, TaskKind};
use toven_testkit::{FakeConfiguredAdapter, FakeProvider, FakeVcsReader, RecordingReporter};

fn mref(ecosystem: &str, name: &str) -> ModuleRef {
    ModuleRef::new(eid(ecosystem), name).expect("valid module ref")
}

fn wsid(id: &str) -> WorkspaceId {
    WorkspaceId::new(id).expect("valid workspace id")
}

fn module(ecosystem: &str, name: &str, root: &str, workspace: &str) -> Module {
    let mut module = Module::new(mref(ecosystem, name), RepoPath::new(root).expect("root"));
    module.workspace = Some(wsid(workspace));
    module
}

/// A two-module rust workspace: `app` depends on `errors`, one Test task.
fn rust_provider() -> FakeProvider {
    let mut response = DiscoverResponse::new(eid("rust"));
    response.workspaces.push(Workspace::new(
        wsid("rust"),
        RepoPath::new(".").expect("root"),
        ToolchainTag::new("cargo"),
    ));
    response
        .modules
        .push(module("rust", "errors", "crates/errors", "rust"));
    response
        .modules
        .push(module("rust", "app", "crates/app", "rust"));
    response.edges.push(Edge::new(
        mref("rust", "app"),
        mref("rust", "errors"),
        DepKind::Normal,
    ));

    let task = Task::new(
        TaskKind::Test,
        vec!["cargo".to_string(), "test".to_string()],
        FanOut::WholeWorkspace,
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
        members: Vec::new(),
    }
}

fn request(intent: TaskKind) -> PlanRequest {
    PlanRequest::new(
        "run-1",
        "toven",
        intent,
        AbsPath::new("/repo").expect("absolute"),
    )
}

#[test]
fn plans_full_federation_into_leaf_first_waves() {
    let provider = rust_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let vcs = FakeVcsReader::new();
    let digest = StubDigest;
    let prober = CountingProber::default();
    let cache = NullCache;
    let mut reporter = RecordingReporter::new();

    let host = PlanHost::new(&vcs, &digest, &prober, &cache);
    let plan = plan(
        &request(TaskKind::Test),
        &document(),
        &providers,
        host,
        &mut reporter,
    )
    .expect("plan succeeds");

    // Two per-module units, leaf (errors) scheduled before its dependent (app).
    assert_eq!(plan.units.len(), 2);
    assert_eq!(
        plan.waves,
        vec![
            vec!["rust:errors#test".to_string()],
            vec!["rust:app#test".to_string()],
        ]
    );

    let app = plan
        .units
        .iter()
        .find(|unit| unit.id == "rust:app#test")
        .expect("app unit present");
    assert_eq!(app.argv, vec!["cargo".to_string(), "test".to_string()]);
    assert_eq!(app.workspace, Some(wsid("rust")));
    assert_eq!(app.kind, "test");

    // One probe for the single active workspace; every unit a miss under NullCache.
    assert_eq!(prober.calls(), 1);
    assert!(
        plan.units
            .iter()
            .all(|unit| unit.cache == CacheVerdict::Miss)
    );
}

#[test]
fn emits_phase_and_plan_events_in_order() {
    let provider = rust_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let vcs = FakeVcsReader::new();
    let digest = StubDigest;
    let prober = CountingProber::default();
    let cache = NullCache;
    let mut reporter = RecordingReporter::new();

    let host = PlanHost::new(&vcs, &digest, &prober, &cache);
    plan(
        &request(TaskKind::Test),
        &document(),
        &providers,
        host,
        &mut reporter,
    )
    .expect("plan succeeds");

    let phases: Vec<Phase> = reporter
        .events()
        .iter()
        .filter_map(|event| match event {
            Event::PhaseStarted { phase } => Some(*phase),
            _ => None,
        })
        .collect();
    assert_eq!(
        phases,
        vec![
            Phase::Configure,
            Phase::Discover,
            Phase::Graph,
            Phase::Affected,
            Phase::Toolchain,
            Phase::Schedule,
        ]
    );

    let prepared = reporter
        .events()
        .iter()
        .any(|event| matches!(event, Event::PlanPrepared { waves: 2, units: 2 }));
    assert!(prepared, "expected PlanPrepared with 2 waves / 2 units");

    let decided = reporter
        .events()
        .iter()
        .filter(|event| matches!(event, Event::CacheDecided { .. }))
        .count();
    assert_eq!(decided, 2);
}

#[test]
fn immutable_plan_round_trips_through_serde() {
    let provider = rust_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let vcs = FakeVcsReader::new();
    let digest = StubDigest;
    let prober = CountingProber::default();
    let cache = NullCache;
    let mut reporter = RecordingReporter::new();

    let host = PlanHost::new(&vcs, &digest, &prober, &cache);
    let plan = plan(
        &request(TaskKind::Test),
        &document(),
        &providers,
        host,
        &mut reporter,
    )
    .expect("plan succeeds");

    let json = serde_json::to_string(&plan).expect("serialize");
    let back: Plan = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(plan, back);
}

#[test]
fn force_mode_marks_every_unit_forced() {
    let provider = rust_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let vcs = FakeVcsReader::new();
    let digest = StubDigest;
    let prober = CountingProber::default();
    let cache = NullCache;
    let mut reporter = RecordingReporter::new();

    let host = PlanHost::new(&vcs, &digest, &prober, &cache);
    let request = request(TaskKind::Test).with_cache_mode(CacheMode::Force);
    let plan = plan(&request, &document(), &providers, host, &mut reporter).expect("plan succeeds");

    assert!(
        plan.units
            .iter()
            .all(|unit| unit.cache == CacheVerdict::Forced)
    );
}

#[test]
fn changed_selection_restricts_active_units() {
    let provider = rust_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    // Only `errors` sources changed; `app` is reached via the reverse closure.
    let vcs = FakeVcsReader::new().with_changed_since(vec![toven_ports::ChangeRecord::new(
        "crates/errors/lib.rs",
        toven_ports::ChangeStatus::Modified,
    )]);
    let digest = StubDigest;
    let prober = CountingProber::default();
    let cache = NullCache;
    let mut reporter = RecordingReporter::new();

    let host = PlanHost::new(&vcs, &digest, &prober, &cache);
    let request = request(TaskKind::Test).with_selection(Selection::Changed(
        toven_ports::BaselineSpec::explicit("main"),
    ));
    let plan = plan(&request, &document(), &providers, host, &mut reporter).expect("plan succeeds");

    let mut ids: Vec<&str> = plan.units.iter().map(|unit| unit.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(ids, vec!["rust:app#test", "rust:errors#test"]);
}

#[test]
fn cache_keys_are_deterministic_and_drive_hits() {
    let provider = rust_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let vcs = FakeVcsReader::new();
    let digest = StubDigest;
    let prober = CountingProber::default();

    // First run: capture the deterministic content keys the plan queries.
    let recording = RecordingCache::default();
    let mut reporter = RecordingReporter::new();
    let host = PlanHost::new(&vcs, &digest, &prober, &recording);
    plan(
        &request(TaskKind::Test),
        &document(),
        &providers,
        host,
        &mut reporter,
    )
    .expect("first plan succeeds");
    let keys = recording.queried();
    assert_eq!(keys.len(), 2, "one key per unit");

    // Second run: seed one captured key — the same key must recur (determinism)
    // and turn exactly that unit into a hit.
    let cache = SetCache::default().with_key(keys[0].clone());
    let mut reporter = RecordingReporter::new();
    let host = PlanHost::new(&vcs, &digest, &prober, &cache);
    let plan = plan(
        &request(TaskKind::Test),
        &document(),
        &providers,
        host,
        &mut reporter,
    )
    .expect("second plan succeeds");

    let hits = plan
        .units
        .iter()
        .filter(|unit| unit.cache == CacheVerdict::Hit)
        .count();
    assert_eq!(hits, 1, "exactly one unit hits the seeded key");
}
