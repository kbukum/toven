//! End-to-end PLAN pipeline tests over fake adapters, VCS, prober, and cache.

mod common;

use std::collections::BTreeMap;

use common::eid;
use toven_engine::config::{Document, GroupConfig, ProjectConfig, TovenConfig};
use toven_engine::federation::MemberVcsReaders;
use toven_engine::federation::resolve::PathDriverLocator;
use toven_engine::plan::{
    CacheMode, NullCache, PlanHost, PlanRequest, Selection, dependency_graph, plan,
};
use toven_model::{
    AbsPath, CacheVerdict, DepKind, Edge, Event, Module, ModuleRef, ModuleSelector, Phase, Plan,
    RepoPath, TaskOrigin, ToolchainTag, Workspace, WorkspaceId,
};
use toven_ports::{DiscoverResponse, FanOut, Provider, Task, TaskIntent, TaskKind, TaskOverride};
use toven_testkit::{
    CountingToolchainProber, FakeCacheStore, FakeConfiguredAdapter, FakeProvider, FakeSourceDigest,
    FakeVcsReader, RecordingCacheStore, RecordingReporter,
};

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
        "test",
        vec!["cargo".to_string(), "test".to_string()],
        FanOut::WholeWorkspace,
    );
    let adapter = FakeConfiguredAdapter::new(eid("rust"))
        .with_response(response)
        .with_tasks(vec![task]);
    FakeProvider::new(eid("rust")).with_adapter(adapter)
}

/// The [`rust_provider`] discovery graph with no configured tasks.
fn rust_provider_without_tasks() -> FakeProvider {
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

    let adapter = FakeConfiguredAdapter::new(eid("rust")).with_response(response);
    FakeProvider::new(eid("rust")).with_adapter(adapter)
}

/// The [`rust_provider`] workspace whose Test task declares one `shared_input`,
/// so the unit key folds (and would hash) that path when caching is active.
fn rust_provider_with_shared_input(path: &str) -> FakeProvider {
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

    let mut task = Task::new(
        "test",
        vec!["cargo".to_string(), "test".to_string()],
        FanOut::WholeWorkspace,
    );
    task.shared_inputs = vec![path.to_string()];
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
    }
}

fn request(intent: TaskIntent) -> PlanRequest {
    PlanRequest::new(
        "run-1",
        "toven",
        intent,
        AbsPath::new("/repo").expect("absolute"),
    )
}

#[test]
fn dependency_graph_does_not_require_a_schedulable_task() {
    let provider = rust_provider_without_tasks();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let mut reporter = RecordingReporter::new();

    let graph = dependency_graph(
        &AbsPath::new("/repo").expect("absolute"),
        &document(),
        &providers,
        &PathDriverLocator::new(),
        &mut reporter,
    )
    .expect("graph succeeds");

    assert_eq!(graph.len(), 2);
    assert_eq!(graph.edges().len(), 1);
    assert_eq!(graph.edges()[0].from.module, mref("rust", "app"));
    assert_eq!(graph.edges()[0].to.module, mref("rust", "errors"));
}

#[test]
fn plans_full_federation_into_leaf_first_waves() {
    let provider = rust_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let vcs = FakeVcsReader::new();
    let digest = FakeSourceDigest::new();
    let prober = CountingToolchainProber::new();
    let cache = NullCache;
    let mut reporter = RecordingReporter::new();

    let readers = MemberVcsReaders::single(&vcs, toven_ports::BaselineSpec::explicit("main"));
    let host = PlanHost::new(&readers, &digest, &prober, &cache);
    let plan = plan(
        &request(TaskIntent::resolve("test")),
        &document(),
        &providers,
        host,
        &mut reporter,
    )
    .expect("plan succeeds");

    // Whole-workspace fan-out collapses both modules into a single invocation.
    assert_eq!(plan.units.len(), 1);
    assert_eq!(plan.waves, vec![vec!["rust@rust#test".to_string()]]);

    let app = plan
        .units
        .iter()
        .find(|unit| unit.id == "rust@rust#test")
        .expect("collapsed unit present");
    assert_eq!(app.members.len(), 2);
    assert_eq!(app.argv, vec!["cargo".to_string(), "test".to_string()]);
    assert_eq!(app.workspace, Some(wsid("rust")));
    assert_eq!(app.task, "test");

    // One probe for the single active workspace; every unit a miss under NullCache.
    assert_eq!(prober.calls(), 1);
    assert!(
        plan.units
            .iter()
            .all(|unit| unit.cache == CacheVerdict::Miss)
    );
}

#[test]
fn group_task_override_splits_members_into_their_own_unit() {
    let provider = rust_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let vcs = FakeVcsReader::new();
    let digest = FakeSourceDigest::new();
    let prober = CountingToolchainProber::new();
    let cache = NullCache;
    let mut reporter = RecordingReporter::new();

    // `integration` overrides the Test argv for `app` only; `errors` keeps the
    // adapter default and stays in the plain whole-workspace batch unit.
    let mut document = document();
    let mut group = GroupConfig {
        modules: vec!["rust:app".to_string()],
        ..GroupConfig::default()
    };
    group.tasks.insert(
        "test".to_string(),
        TaskOverride {
            argv: Some(vec!["cargo".to_string(), "nextest".to_string()]),
            ..TaskOverride::default()
        },
    );
    document.groups.insert("integration".to_string(), group);

    let readers = MemberVcsReaders::single(&vcs, toven_ports::BaselineSpec::explicit("main"));
    let host = PlanHost::new(&readers, &digest, &prober, &cache);
    let plan = plan(
        &request(TaskIntent::resolve("test")),
        &document,
        &providers,
        host,
        &mut reporter,
    )
    .expect("plan succeeds");

    assert_eq!(plan.units.len(), 2);
    let overridden = plan
        .units
        .iter()
        .find(|unit| unit.id == "rust@rust~integration#test")
        .expect("group-tagged unit present");
    assert_eq!(
        overridden.argv,
        vec!["cargo".to_string(), "nextest".to_string()]
    );
    assert_eq!(overridden.members, vec![mref("rust", "app").into()]);
    assert_eq!(overridden.origin, TaskOrigin::Group);

    let default = plan
        .units
        .iter()
        .find(|unit| unit.id == "rust@rust#test")
        .expect("default unit present");
    assert_eq!(default.argv, vec!["cargo".to_string(), "test".to_string()]);
    assert_eq!(default.members, vec![mref("rust", "errors").into()]);
    assert_eq!(default.origin, TaskOrigin::Project);
}

#[test]
fn conflicting_group_task_overrides_are_rejected() {
    let provider = rust_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let vcs = FakeVcsReader::new();
    let digest = FakeSourceDigest::new();
    let prober = CountingToolchainProber::new();
    let cache = NullCache;
    let mut reporter = RecordingReporter::new();

    // Both groups claim `app` and override `test`: an explicit, fail-closed error
    // rather than an implicit last-writer-wins.
    let override_group = |argv: &str| {
        let mut group = GroupConfig {
            modules: vec!["rust:app".to_string()],
            ..GroupConfig::default()
        };
        group.tasks.insert(
            "test".to_string(),
            TaskOverride {
                argv: Some(vec![argv.to_string()]),
                ..TaskOverride::default()
            },
        );
        group
    };
    let mut document = document();
    document
        .groups
        .insert("first".to_string(), override_group("nextest"));
    document
        .groups
        .insert("second".to_string(), override_group("cargo-test"));

    let readers = MemberVcsReaders::single(&vcs, toven_ports::BaselineSpec::explicit("main"));
    let host = PlanHost::new(&readers, &digest, &prober, &cache);
    let error = plan(
        &request(TaskIntent::resolve("test")),
        &document,
        &providers,
        host,
        &mut reporter,
    )
    .expect_err("conflicting group overrides rejected");
    assert!(error.to_string().contains("conflicting groups"), "{error}");
}

#[test]
fn emits_phase_and_plan_events_in_order() {
    let provider = rust_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let vcs = FakeVcsReader::new();
    let digest = FakeSourceDigest::new();
    let prober = CountingToolchainProber::new();
    let cache = NullCache;
    let mut reporter = RecordingReporter::new();

    let readers = MemberVcsReaders::single(&vcs, toven_ports::BaselineSpec::explicit("main"));
    let host = PlanHost::new(&readers, &digest, &prober, &cache);
    plan(
        &request(TaskIntent::resolve("test")),
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
        .any(|event| matches!(event, Event::PlanPrepared { waves: 1, units: 1 }));
    assert!(prepared, "expected PlanPrepared with 1 wave / 1 unit");

    let decided = reporter
        .events()
        .iter()
        .filter(|event| matches!(event, Event::CacheDecided { .. }))
        .count();
    assert_eq!(decided, 1);
}

#[test]
fn immutable_plan_round_trips_through_serde() {
    let provider = rust_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let vcs = FakeVcsReader::new();
    let digest = FakeSourceDigest::new();
    let prober = CountingToolchainProber::new();
    let cache = NullCache;
    let mut reporter = RecordingReporter::new();

    let readers = MemberVcsReaders::single(&vcs, toven_ports::BaselineSpec::explicit("main"));
    let host = PlanHost::new(&readers, &digest, &prober, &cache);
    let plan = plan(
        &request(TaskIntent::resolve("test")),
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
fn disabled_cache_skips_unit_key_so_unreadable_shared_input_never_aborts_plan() {
    // With caching disabled every unit's verdict is statically `Disabled`, so no
    // content key is needed. A shared input whose digest read would fail must
    // therefore never be consulted: lazy key computation keeps PLAN succeeding
    // instead of surfacing an avoidable digest error for an uncacheable unit.
    let provider = rust_provider_with_shared_input("unreadable.lock");
    let providers: Vec<&dyn Provider> = vec![&provider];
    let vcs = FakeVcsReader::new();
    let digest = FakeSourceDigest::new().with_failing_path("unreadable.lock");
    let prober = CountingToolchainProber::new();
    let cache = NullCache;
    let mut reporter = RecordingReporter::new();

    let readers = MemberVcsReaders::single(&vcs, toven_ports::BaselineSpec::explicit("main"));
    let host = PlanHost::new(&readers, &digest, &prober, &cache);
    let request = request(TaskIntent::resolve("test")).with_cache_mode(CacheMode::Disabled);
    let plan = plan(&request, &document(), &providers, host, &mut reporter).expect("plan succeeds");

    assert!(
        plan.units
            .iter()
            .all(|unit| { unit.cache == CacheVerdict::Disabled && unit.cache_key.is_none() })
    );
}

#[test]
fn uncacheable_task_is_statically_disabled_even_with_caching_active() {
    // A mutating `*-fix` task authors `cacheable = false`. Even under an active
    // cache mode its unit must be statically `Disabled` (no key recorded), so a
    // stale content-key hit can never suppress the mutation on a later run.
    let mut response = DiscoverResponse::new(eid("rust"));
    response.workspaces.push(Workspace::new(
        wsid("rust"),
        RepoPath::new(".").expect("root"),
        ToolchainTag::new("cargo"),
    ));
    response
        .modules
        .push(module("rust", "errors", "crates/errors", "rust"));
    let mut task = Task::new(
        "format-fix",
        vec!["cargo".to_string(), "fmt".to_string()],
        FanOut::WholeWorkspace,
    );
    task.cacheable = false;
    let adapter = FakeConfiguredAdapter::new(eid("rust"))
        .with_response(response)
        .with_tasks(vec![task]);
    let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
    let providers: Vec<&dyn Provider> = vec![&provider];

    let vcs = FakeVcsReader::new();
    let digest = FakeSourceDigest::new();
    let prober = CountingToolchainProber::new();
    let cache = NullCache;
    let mut reporter = RecordingReporter::new();

    let readers = MemberVcsReaders::single(&vcs, toven_ports::BaselineSpec::explicit("main"));
    let host = PlanHost::new(&readers, &digest, &prober, &cache);
    // An active read/write cache mode: only the task's own opt-out should disable it.
    let request = request(TaskIntent::resolve("format-fix"));
    let plan = plan(&request, &document(), &providers, host, &mut reporter).expect("plan succeeds");

    assert!(
        plan.units
            .iter()
            .all(|unit| unit.cache == CacheVerdict::Disabled && unit.cache_key.is_none()),
        "an uncacheable task's units are statically Disabled",
    );
}

#[test]
fn force_mode_marks_every_unit_forced() {
    let provider = rust_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let vcs = FakeVcsReader::new();
    let digest = FakeSourceDigest::new();
    let prober = CountingToolchainProber::new();
    let cache = NullCache;
    let mut reporter = RecordingReporter::new();

    let readers = MemberVcsReaders::single(&vcs, toven_ports::BaselineSpec::explicit("main"));
    let host = PlanHost::new(&readers, &digest, &prober, &cache);
    let request = request(TaskIntent::resolve("test")).with_cache_mode(CacheMode::Force);
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
    let digest = FakeSourceDigest::new();
    let prober = CountingToolchainProber::new();
    let cache = NullCache;
    let mut reporter = RecordingReporter::new();

    let readers = MemberVcsReaders::single(&vcs, toven_ports::BaselineSpec::explicit("main"));
    let host = PlanHost::new(&readers, &digest, &prober, &cache);
    let request = request(TaskIntent::resolve("test")).with_selection(Selection::Changed(Some(
        toven_ports::BaselineSpec::explicit("main"),
    )));
    let plan = plan(&request, &document(), &providers, host, &mut reporter).expect("plan succeeds");

    let mut ids: Vec<&str> = plan.units.iter().map(|unit| unit.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(ids, vec!["rust@rust#test"]);
}

#[test]
fn cache_keys_are_deterministic_and_drive_hits() {
    let provider = rust_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let vcs = FakeVcsReader::new();
    let digest = FakeSourceDigest::new();
    let prober = CountingToolchainProber::new();

    // First run: capture the deterministic content keys the plan queries.
    let recording = RecordingCacheStore::new();
    let mut reporter = RecordingReporter::new();
    let readers = MemberVcsReaders::single(&vcs, toven_ports::BaselineSpec::explicit("main"));
    let host = PlanHost::new(&readers, &digest, &prober, &recording);
    plan(
        &request(TaskIntent::resolve("test")),
        &document(),
        &providers,
        host,
        &mut reporter,
    )
    .expect("first plan succeeds");
    let keys = recording.queried();
    assert_eq!(keys.len(), 1, "one key per unit");

    // Second run: seed one captured key — the same key must recur (determinism)
    // and turn exactly that unit into a hit.
    let cache = FakeCacheStore::new().with_key(keys[0].clone());
    let mut reporter = RecordingReporter::new();
    let readers = MemberVcsReaders::single(&vcs, toven_ports::BaselineSpec::explicit("main"));
    let host = PlanHost::new(&readers, &digest, &prober, &cache);
    let plan = plan(
        &request(TaskIntent::resolve("test")),
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

/// A two-module rust workspace where `app` **dev**-depends on `errors`, exposing
/// a renamed test task (`my-test`, `kind = "test"`) and a plain custom task
/// (`deploy`, no recognized kind), each fanning per module.
fn dev_dep_provider() -> FakeProvider {
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
        DepKind::Dev,
    ));

    let my_test = Task::new(
        "my-test",
        vec!["cargo".to_string(), "nextest".to_string()],
        FanOut::PerModule,
    )
    .with_kind(TaskKind::Test);
    let deploy = Task::new("deploy", vec!["./deploy.sh".to_string()], FanOut::PerModule);
    let adapter = FakeConfiguredAdapter::new(eid("rust"))
        .with_response(response)
        .with_tasks(vec![my_test, deploy]);
    FakeProvider::new(eid("rust")).with_adapter(adapter)
}

/// `--dependencies` on the renamed test task must pull the dev-only prerequisite
/// into the plan: recognition reads the task's configured `kind = "test"`, not
/// the typed token, so a renamed test still propagates dev edges.
#[test]
fn renamed_test_task_propagates_dev_edges_by_its_configured_kind() {
    let provider = dev_dep_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let vcs = FakeVcsReader::new();
    let digest = FakeSourceDigest::new();
    let prober = CountingToolchainProber::new();
    let cache = NullCache;
    let mut reporter = RecordingReporter::new();

    let readers = MemberVcsReaders::single(&vcs, toven_ports::BaselineSpec::explicit("main"));
    let host = PlanHost::new(&readers, &digest, &prober, &cache);
    let request = request(TaskIntent::resolve("my-test")).with_selection(Selection::Explicit {
        targets: vec![ModuleSelector::parse("rust:app").expect("selector")],
        include_dependents: false,
        include_dependencies: true,
    });
    let plan = plan(&request, &document(), &providers, host, &mut reporter).expect("plan succeeds");

    let modules: std::collections::BTreeSet<&str> = plan
        .units
        .iter()
        .flat_map(|unit| unit.members.iter().map(|key| key.module.name.as_str()))
        .collect();
    assert!(
        modules.contains("errors"),
        "the dev-only prerequisite is pulled in: {modules:?}"
    );
    // The task is addressable by its renamed name: its own argv is rendered.
    assert!(
        plan.units
            .iter()
            .any(|unit| unit.argv == ["cargo".to_string(), "nextest".to_string()]),
        "the renamed task's argv is scheduled"
    );
}

/// A plain named task with no recognized kind gets default (non-Test) edge
/// semantics: `--dependencies` does not cross the dev-only edge.
#[test]
fn plain_task_does_not_propagate_dev_edges() {
    let provider = dev_dep_provider();
    let providers: Vec<&dyn Provider> = vec![&provider];
    let vcs = FakeVcsReader::new();
    let digest = FakeSourceDigest::new();
    let prober = CountingToolchainProber::new();
    let cache = NullCache;
    let mut reporter = RecordingReporter::new();

    let readers = MemberVcsReaders::single(&vcs, toven_ports::BaselineSpec::explicit("main"));
    let host = PlanHost::new(&readers, &digest, &prober, &cache);
    let request = request(TaskIntent::resolve("deploy")).with_selection(Selection::Explicit {
        targets: vec![ModuleSelector::parse("rust:app").expect("selector")],
        include_dependents: false,
        include_dependencies: true,
    });
    let plan = plan(&request, &document(), &providers, host, &mut reporter).expect("plan succeeds");

    let modules: std::collections::BTreeSet<&str> = plan
        .units
        .iter()
        .flat_map(|unit| unit.members.iter().map(|key| key.module.name.as_str()))
        .collect();
    assert_eq!(
        modules,
        std::collections::BTreeSet::from(["app"]),
        "a plain task does not cross the dev-only edge: {modules:?}"
    );
}

/// Two ecosystems that tag the **same** task name with conflicting recognized
/// kinds: `rust` calls `verify` a `test`, `go` calls it a `build`.
fn conflicting_kind_providers() -> (FakeProvider, FakeProvider) {
    let mut rust = DiscoverResponse::new(eid("rust"));
    rust.workspaces.push(Workspace::new(
        wsid("rust"),
        RepoPath::new(".").expect("root"),
        ToolchainTag::new("cargo"),
    ));
    rust.modules
        .push(module("rust", "app", "crates/app", "rust"));
    let rust_verify = Task::new(
        "verify",
        vec!["cargo".to_string(), "test".to_string()],
        FanOut::PerModule,
    )
    .with_kind(TaskKind::Test);
    let rust_adapter = FakeConfiguredAdapter::new(eid("rust"))
        .with_response(rust)
        .with_tasks(vec![rust_verify]);

    let mut go = DiscoverResponse::new(eid("go"));
    go.workspaces.push(Workspace::new(
        wsid("go"),
        RepoPath::new(".").expect("root"),
        ToolchainTag::new("go"),
    ));
    go.modules.push(module("go", "svc", "services/svc", "go"));
    let go_verify = Task::new(
        "verify",
        vec!["go".to_string(), "build".to_string()],
        FanOut::PerModule,
    )
    .with_kind(TaskKind::Build);
    let go_adapter = FakeConfiguredAdapter::new(eid("go"))
        .with_response(go)
        .with_tasks(vec![go_verify]);

    (
        FakeProvider::new(eid("rust")).with_adapter(rust_adapter),
        FakeProvider::new(eid("go")).with_adapter(go_adapter),
    )
}

/// Recognition is order-independent: when two ecosystems configure the same task
/// name with different kinds the plan fails closed with an actionable error,
/// rather than resolving the kind by arbitrary adapter iteration order.
#[test]
fn conflicting_task_kinds_across_ecosystems_are_rejected() {
    let (rust, go) = conflicting_kind_providers();
    let providers: Vec<&dyn Provider> = vec![&rust, &go];
    let vcs = FakeVcsReader::new();
    let digest = FakeSourceDigest::new();
    let prober = CountingToolchainProber::new();
    let cache = NullCache;
    let mut reporter = RecordingReporter::new();

    let mut document = document();
    document.ecosystems.insert(eid("go"), serde_json::json!({}));

    let readers = MemberVcsReaders::single(&vcs, toven_ports::BaselineSpec::explicit("main"));
    let host = PlanHost::new(&readers, &digest, &prober, &cache);
    let error = plan(
        &request(TaskIntent::resolve("verify")),
        &document,
        &providers,
        host,
        &mut reporter,
    )
    .expect_err("conflicting cross-ecosystem kinds rejected");
    let message = error.to_string();
    assert!(message.contains("conflicting kinds"), "{message}");
    assert!(message.contains("verify"), "{message}");
}
