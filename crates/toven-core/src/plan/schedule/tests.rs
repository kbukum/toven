//! Behavioral tests for the schedule phase: ordering, task resolution, batch
//! grouping with dependency-layer folding, and unit rendering.

use std::collections::BTreeMap;

use toven_model::{
    AbsPath, DepKind, EcosystemId, Edge, Module, ModuleKey, ModuleRef, RepoPath, ToolchainTag, Workspace,
    WorkspaceId,
};
use toven_ports::{
    ConfiguredAdapter, DiscoverResponse, FanOut, RunStrategy, Task, TaskIntent, TaskKind,
    TaskOrigin, TaskOverride,
};
use toven_testkit::FakeConfiguredAdapter;

use super::super::configure::{ConfiguredSet, MemberAdapters};
use super::super::overrides::GroupOverrides;
use super::entry::{Scheduled, schedule};
use super::task::unknown_task_error;
use crate::config::GroupConfig;
use crate::plan::discover::Federation;
use crate::plan::request::PlanRequest;

#[test]
fn unknown_task_error_suggests_the_nearest_name_and_discovery_hint() {
    let available = vec![
        Task::new(
            "format",
            vec!["cargo".into(), "fmt".into()],
            FanOut::WholeWorkspace,
        ),
        Task::new(
            "test",
            vec!["cargo".into(), "test".into()],
            FanOut::Batchable,
        ),
    ];
    let error = unknown_task_error("rust", &TaskIntent::resolve("fmt"), &available);
    let message = error.to_string();
    assert!(message.contains("has no 'fmt' task"), "{message}");
    assert!(message.contains("Did you mean 'format'?"), "{message}");
    assert!(message.contains("toven tasks"), "{message}");
}

#[test]
fn unknown_task_error_omits_a_suggestion_for_a_far_off_name() {
    let available = vec![Task::new(
        "test",
        vec!["cargo".into(), "test".into()],
        FanOut::Batchable,
    )];
    let error = unknown_task_error("rust", &TaskIntent::resolve("zzzzzz"), &available);
    let message = error.to_string();
    assert!(!message.contains("Did you mean"), "{message}");
    assert!(message.contains("toven tasks"), "{message}");
}

fn eid(id: &str) -> EcosystemId {
    EcosystemId::new(id).unwrap()
}

fn mref(ecosystem: &str, name: &str) -> ModuleRef {
    ModuleRef::new(eid(ecosystem), name).unwrap()
}

fn module(ecosystem: &str, name: &str, workspace: &str) -> Module {
    let mut module = Module::new(mref(ecosystem, name), RepoPath::new(name).unwrap());
    module.workspace = Some(WorkspaceId::new(workspace).unwrap());
    module
}

fn workspace(id: &str) -> Workspace {
    Workspace::new(
        WorkspaceId::new(id).unwrap(),
        RepoPath::new(".").unwrap(),
        ToolchainTag::new("cargo"),
    )
}

fn adapter(ecosystem: &str, strategy: RunStrategy) -> Box<dyn ConfiguredAdapter> {
    adapter_with(ecosystem, strategy, FanOut::PerModule)
}

fn adapter_with(
    ecosystem: &str,
    strategy: RunStrategy,
    fan_out: FanOut,
) -> Box<dyn ConfiguredAdapter> {
    // Real ecosystem adapters mark their whole-workspace tool invocations as
    // resolving their own cross-workspace closure, so mirror that here: a
    // whole-workspace `test` task is co-schedulable inside a facade cycle.
    adapter_with_closure(
        ecosystem,
        strategy,
        fan_out,
        fan_out == FanOut::WholeWorkspace,
    )
}

fn adapter_with_closure(
    ecosystem: &str,
    strategy: RunStrategy,
    fan_out: FanOut,
    workspace_closure: bool,
) -> Box<dyn ConfiguredAdapter> {
    let mut task = Task::new("test", vec!["x".to_string()], fan_out);
    task.workspace_closure = workspace_closure;
    Box::new(
        FakeConfiguredAdapter::new(eid(ecosystem))
            .with_response(DiscoverResponse::new(eid(ecosystem)))
            .with_tasks(vec![task])
            .with_run_strategy(strategy),
    )
}

fn request() -> PlanRequest {
    request_for(TaskIntent::resolve("test"))
}

fn request_for(intent: TaskIntent) -> PlanRequest {
    PlanRequest::new("r", "t", intent, AbsPath::new("/repo").unwrap())
}

/// An adapter exposing both a plain `test` task and a `test-integration` named
/// extra (`kind = "test"`) with distinct argv, so a test can prove each
/// resolves by its own user-addressable name.
fn named_extra_adapter(ecosystem: &str) -> Box<dyn ConfiguredAdapter> {
    let plain = Task::new(
        "test",
        vec!["plain".to_string(), "test".to_string()],
        FanOut::PerModule,
    );
    let extra = Task::new(
        "test-integration",
        vec!["integration".to_string(), "test".to_string()],
        FanOut::PerModule,
    )
    .with_kind(TaskKind::Test);
    Box::new(
        FakeConfiguredAdapter::new(eid(ecosystem))
            .with_response(DiscoverResponse::new(eid(ecosystem)))
            .with_tasks(vec![plain, extra])
            .with_run_strategy(RunStrategy::Unordered),
    )
}

fn toolchains(federation: &Federation) -> BTreeMap<WorkspaceId, ToolchainTag> {
    federation
        .workspaces
        .iter()
        .map(|workspace| {
            (
                workspace.id.clone(),
                workspace.toolchain.clone().with_version("v1"),
            )
        })
        .collect()
}

fn single_member(set: ConfiguredSet) -> MemberAdapters {
    let mut adapters = MemberAdapters::default();
    adapters.insert(None, set);
    adapters
}

fn waves_for(federation: &Federation, adapters: &MemberAdapters) -> Vec<Vec<String>> {
    let active: Vec<toven_model::ModuleKey> = federation.modules.iter().map(Module::key).collect();
    schedule(
        &request(),
        federation,
        &active,
        adapters,
        &GroupOverrides::default(),
        &toolchains(federation),
    )
    .unwrap()
    .waves
}

/// The sorted unit ids of a scheduled result.
fn ids(scheduled: &Scheduled) -> Vec<String> {
    let mut ids: Vec<String> = scheduled.units.iter().map(|unit| unit.id.clone()).collect();
    ids.sort_unstable();
    ids
}

#[test]
fn workspace_module_without_resolved_toolchain_is_rejected() {
    let federation = Federation {
        workspaces: vec![workspace("rust")],
        modules: vec![module("rust", "app", "rust")],
        edges: Vec::new(),
        warnings: Vec::new(),
    };
    let mut adapters = ConfiguredSet::new();
    adapters.insert(eid("rust"), adapter("rust", RunStrategy::Unordered));
    let adapters = single_member(adapters);

    let active = vec![toven_model::ModuleKey::bare(mref("rust", "app"))];
    // Empty toolchain map: the workspace-owning module has no resolved identity,
    // which must fail closed rather than key against an empty one.
    let result = schedule(
        &request(),
        &federation,
        &active,
        &adapters,
        &GroupOverrides::default(),
        &BTreeMap::new(),
    );
    assert!(result.is_err());
}

#[test]
fn leaf_to_top_orders_dependencies_before_dependents() {
    let federation = Federation {
        workspaces: vec![workspace("rust")],
        modules: vec![
            module("rust", "app", "rust"),
            module("rust", "errors", "rust"),
        ],
        edges: vec![Edge::new(
            mref("rust", "app"),
            mref("rust", "errors"),
            DepKind::Normal,
        )],
        warnings: Vec::new(),
    };
    let mut adapters = ConfiguredSet::new();
    adapters.insert(eid("rust"), adapter("rust", RunStrategy::LeafToTop));

    assert_eq!(
        waves_for(&federation, &single_member(adapters)),
        vec![
            vec!["rust:errors#test".to_string()],
            vec!["rust:app#test".to_string()],
        ]
    );
}

#[test]
fn unordered_collapses_intra_ecosystem_edges_into_one_wave() {
    let federation = Federation {
        workspaces: vec![workspace("rust")],
        modules: vec![
            module("rust", "app", "rust"),
            module("rust", "errors", "rust"),
        ],
        edges: vec![Edge::new(
            mref("rust", "app"),
            mref("rust", "errors"),
            DepKind::Normal,
        )],
        warnings: Vec::new(),
    };
    let mut adapters = ConfiguredSet::new();
    adapters.insert(eid("rust"), adapter("rust", RunStrategy::Unordered));

    assert_eq!(
        waves_for(&federation, &single_member(adapters)),
        vec![vec![
            "rust:app#test".to_string(),
            "rust:errors#test".to_string()
        ]]
    );
}

#[test]
fn overlay_edges_are_never_dropped_even_under_unordered() {
    let federation = Federation {
        workspaces: vec![workspace("go"), workspace("rust")],
        modules: vec![module("go", "api", "go"), module("rust", "shared", "rust")],
        edges: vec![Edge::new(
            mref("go", "api"),
            mref("rust", "shared"),
            DepKind::Overlay,
        )],
        warnings: Vec::new(),
    };
    let mut adapters = ConfiguredSet::new();
    adapters.insert(eid("go"), adapter("go", RunStrategy::Unordered));
    adapters.insert(eid("rust"), adapter("rust", RunStrategy::Unordered));

    // The overlay still orders shared before api despite both being unordered.
    assert_eq!(
        waves_for(&federation, &single_member(adapters)),
        vec![
            vec!["rust:shared#test".to_string()],
            vec!["go:api#test".to_string()],
        ]
    );
}

#[test]
fn whole_workspace_collapses_modules_into_one_unit() {
    let federation = Federation {
        workspaces: vec![workspace("rust")],
        modules: vec![
            module("rust", "app", "rust"),
            module("rust", "errors", "rust"),
        ],
        edges: Vec::new(),
        warnings: Vec::new(),
    };
    let mut adapters = ConfiguredSet::new();
    adapters.insert(
        eid("rust"),
        adapter_with("rust", RunStrategy::Unordered, FanOut::WholeWorkspace),
    );
    assert_eq!(
        waves_for(&federation, &single_member(adapters)),
        vec![vec!["rust@rust#test".to_string()]]
    );
}

#[test]
fn batchable_splits_distinct_workspaces_in_one_ecosystem() {
    // Two Cargo workspaces under the same ecosystem must not collapse into one
    // batched unit: each unit's {workspace.root}/toolchain comes from its
    // representative, so a cross-workspace collapse would mis-render the others.
    let federation = Federation {
        workspaces: vec![workspace("core"), workspace("contrib")],
        modules: vec![
            module("rust", "errors", "core"),
            module("rust", "plugin", "contrib"),
        ],
        edges: Vec::new(),
        warnings: Vec::new(),
    };
    let mut adapters = ConfiguredSet::new();
    adapters.insert(
        eid("rust"),
        adapter_with("rust", RunStrategy::Unordered, FanOut::Batchable),
    );
    let active: Vec<toven_model::ModuleKey> = federation.modules.iter().map(Module::key).collect();
    let scheduled = schedule(
        &request(),
        &federation,
        &active,
        &single_member(adapters),
        &GroupOverrides::default(),
        &toolchains(&federation),
    )
    .unwrap();
    assert_eq!(scheduled.units.len(), 2);
    let mut ids: Vec<&str> = scheduled
        .units
        .iter()
        .map(|unit| unit.id.as_str())
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, vec!["rust@contrib#test", "rust@core#test"]);
    assert!(scheduled.units.iter().all(|unit| unit.members.len() == 1));
}

#[test]
fn batchable_groups_members_and_keeps_distinct_ecosystems_apart() {
    let federation = Federation {
        workspaces: vec![workspace("go"), workspace("rust")],
        modules: vec![
            module("rust", "app", "rust"),
            module("rust", "errors", "rust"),
            module("go", "api", "go"),
        ],
        edges: Vec::new(),
        warnings: Vec::new(),
    };
    let mut adapters = ConfiguredSet::new();
    adapters.insert(
        eid("rust"),
        adapter_with("rust", RunStrategy::Unordered, FanOut::Batchable),
    );
    adapters.insert(
        eid("go"),
        adapter_with("go", RunStrategy::Unordered, FanOut::Batchable),
    );
    let active: Vec<toven_model::ModuleKey> = federation.modules.iter().map(Module::key).collect();
    let scheduled = schedule(
        &request(),
        &federation,
        &active,
        &single_member(adapters),
        &GroupOverrides::default(),
        &toolchains(&federation),
    )
    .unwrap();
    assert_eq!(scheduled.units.len(), 2);
    let rust = scheduled
        .units
        .iter()
        .find(|unit| unit.id == "rust@rust#test")
        .unwrap();
    assert_eq!(rust.members.len(), 2);
}

#[test]
fn batchable_single_workspace_chain_stays_one_batched_unit() {
    // A clean single Cargo workspace with an internal dependency chain (app →
    // corelib → util) under a batchable task. The modules span three dependency
    // layers, but they form no cross-group cycle — every edge is intra-group — so
    // the base must stay a single `cargo check -p …` unit rather than fragmenting
    // into one unit per layer.
    let federation = Federation {
        workspaces: vec![workspace("rust")],
        modules: vec![
            module("rust", "app", "rust"),
            module("rust", "corelib", "rust"),
            module("rust", "util", "rust"),
        ],
        edges: vec![
            Edge::new(
                mref("rust", "app"),
                mref("rust", "corelib"),
                DepKind::Normal,
            ),
            Edge::new(
                mref("rust", "corelib"),
                mref("rust", "util"),
                DepKind::Normal,
            ),
        ],
        warnings: Vec::new(),
    };
    let mut adapters = ConfiguredSet::new();
    adapters.insert(
        eid("rust"),
        adapter_with("rust", RunStrategy::LeafToTop, FanOut::Batchable),
    );
    let active: Vec<toven_model::ModuleKey> = federation.modules.iter().map(Module::key).collect();
    let scheduled = schedule(
        &request(),
        &federation,
        &active,
        &single_member(adapters),
        &GroupOverrides::default(),
        &toolchains(&federation),
    )
    .unwrap();

    assert_eq!(ids(&scheduled), vec!["rust@rust#test".to_string()]);
    let unit = &scheduled.units[0];
    assert_eq!(unit.members.len(), 3);
    assert!(unit.depends_on.is_empty());
    assert_eq!(scheduled.waves, vec![vec!["rust@rust#test".to_string()]]);
}

#[test]
fn external_dependent_waves_after_an_unsplit_multi_layer_batch() {
    // An un-split multi-layer batch (`rust` workspace `core0 ← core1`, one batched
    // unit spanning layers 0 and 1) that an external unit depends on. `go:api`
    // overlays onto `core0`, the batch's *leading* layer, so at the module level
    // `api` shares `core1`'s wave. Deriving unit waves from the collapsed unit
    // graph — not member module wave-indices — must still place the whole batch
    // before `api`, since `api` gates on the single unit that builds `core0`.
    let federation = Federation {
        workspaces: vec![workspace("rust"), workspace("go")],
        modules: vec![
            module("rust", "core0", "rust"),
            module("rust", "core1", "rust"),
            module("go", "api", "go"),
        ],
        edges: vec![
            Edge::new(
                mref("rust", "core1"),
                mref("rust", "core0"),
                DepKind::Normal,
            ),
            Edge::new(mref("go", "api"), mref("rust", "core0"), DepKind::Overlay),
        ],
        warnings: Vec::new(),
    };
    let mut adapters = ConfiguredSet::new();
    adapters.insert(
        eid("rust"),
        adapter_with("rust", RunStrategy::LeafToTop, FanOut::Batchable),
    );
    adapters.insert(
        eid("go"),
        adapter_with("go", RunStrategy::Unordered, FanOut::Batchable),
    );
    let active: Vec<toven_model::ModuleKey> = federation.modules.iter().map(Module::key).collect();
    let scheduled = schedule(
        &request(),
        &federation,
        &active,
        &single_member(adapters),
        &GroupOverrides::default(),
        &toolchains(&federation),
    )
    .unwrap();

    // The rust batch stays one unit (no cross-group cycle) spanning both layers;
    // `api` gates on it and lands strictly after — no inversion.
    let rust = scheduled
        .units
        .iter()
        .find(|unit| unit.id == "rust@rust#test")
        .unwrap();
    assert_eq!(rust.members.len(), 2);
    let api = scheduled
        .units
        .iter()
        .find(|unit| unit.id == "go@go#test")
        .unwrap();
    assert_eq!(api.depends_on, vec!["rust@rust#test".to_string()]);
    assert_eq!(
        scheduled.waves,
        vec![
            vec!["rust@rust#test".to_string()],
            vec!["go@go#test".to_string()],
        ]
    );
}

#[test]
fn facade_back_dependency_splits_a_workspace_across_layers_into_a_dag() {
    // `rskit`-shaped facade back-dependency: the `core` workspace holds a base
    // crate and a suite/facade crate; a `contrib` module depends on core's base,
    // while core's suite depends back on that contrib module. Per-workspace-only
    // batching would collapse core into one super-node and make the core⇄contrib
    // unit graph cyclic. Layer-aware grouping splits core across its two layers,
    // yielding an acyclic DAG: base → contrib → suite. An independent `examples`
    // module shares base's leading wave.
    let federation = Federation {
        workspaces: vec![
            workspace("core"),
            workspace("contrib"),
            workspace("examples"),
        ],
        modules: vec![
            module("rust", "base", "core"),
            module("rust", "suite", "core"),
            module("rust", "plugin", "contrib"),
            module("rust", "sample", "examples"),
        ],
        edges: vec![
            Edge::new(
                mref("rust", "plugin"),
                mref("rust", "base"),
                DepKind::Normal,
            ),
            Edge::new(
                mref("rust", "suite"),
                mref("rust", "plugin"),
                DepKind::Normal,
            ),
        ],
        warnings: Vec::new(),
    };
    let mut adapters = ConfiguredSet::new();
    adapters.insert(
        eid("rust"),
        adapter_with("rust", RunStrategy::LeafToTop, FanOut::Batchable),
    );
    let active: Vec<toven_model::ModuleKey> = federation.modules.iter().map(Module::key).collect();
    let scheduled = schedule(
        &request(),
        &federation,
        &active,
        &single_member(adapters),
        &GroupOverrides::default(),
        &toolchains(&federation),
    )
    .unwrap();

    let unit = |id: &str| {
        scheduled
            .units
            .iter()
            .find(|unit| unit.id == id)
            .unwrap_or_else(|| panic!("missing unit '{id}': {:?}", ids(&scheduled)))
    };

    // core is split into a base layer (L0) and a suite layer (L2); contrib and the
    // independent examples workspace stay single-layer.
    assert_eq!(
        ids(&scheduled),
        vec![
            "rust@contrib#test".to_string(),
            "rust@core~~L0#test".to_string(),
            "rust@core~~L2#test".to_string(),
            "rust@examples#test".to_string(),
        ]
    );

    // Acyclic depends_on: base has none, contrib gates on base, suite gates on
    // contrib — and nothing gates back onto suite (no core⇄contrib cycle).
    assert!(unit("rust@core~~L0#test").depends_on.is_empty());
    assert_eq!(
        unit("rust@contrib#test").depends_on,
        vec!["rust@core~~L0#test".to_string()]
    );
    assert_eq!(
        unit("rust@core~~L2#test").depends_on,
        vec!["rust@contrib#test".to_string()]
    );

    // Three waves, base layer first; the independent examples workspace shares that
    // leading wave.
    assert_eq!(scheduled.waves.len(), 3);
    let mut first = scheduled.waves[0].clone();
    first.sort_unstable();
    assert_eq!(
        first,
        vec![
            "rust@core~~L0#test".to_string(),
            "rust@examples#test".to_string(),
        ]
    );
    assert_eq!(scheduled.waves[1], vec!["rust@contrib#test".to_string()]);
    assert_eq!(scheduled.waves[2], vec!["rust@core~~L2#test".to_string()]);
}

#[test]
fn whole_workspace_facade_cycle_co_schedules_the_cycle_into_one_wave() {
    // The same facade back-dependency shape under `WholeWorkspace` fan-out: `core`
    // collapses into one indivisible unit covering both `base` and `suite`. With
    // `suite` → `plugin` → `base`, the core and contrib units mutually depend — a
    // cycle no layer split can break (a whole-workspace invocation cannot run half
    // a workspace). Each atomic whole-workspace invocation resolves its own
    // path-dependency closure, so the two units have no real build handoff and are
    // safe to co-schedule: the leveler condenses the strongly-connected component
    // into a single wave instead of failing closed.
    let federation = Federation {
        workspaces: vec![workspace("core"), workspace("contrib")],
        modules: vec![
            module("rust", "base", "core"),
            module("rust", "suite", "core"),
            module("rust", "plugin", "contrib"),
        ],
        edges: vec![
            Edge::new(
                mref("rust", "plugin"),
                mref("rust", "base"),
                DepKind::Normal,
            ),
            Edge::new(
                mref("rust", "suite"),
                mref("rust", "plugin"),
                DepKind::Normal,
            ),
        ],
        warnings: Vec::new(),
    };
    let mut adapters = ConfiguredSet::new();
    adapters.insert(
        eid("rust"),
        adapter_with("rust", RunStrategy::LeafToTop, FanOut::WholeWorkspace),
    );
    let active: Vec<toven_model::ModuleKey> = federation.modules.iter().map(Module::key).collect();
    let scheduled = schedule(
        &request(),
        &federation,
        &active,
        &single_member(adapters),
        &GroupOverrides::default(),
        &toolchains(&federation),
    )
    .expect("an irreducible whole-workspace cycle co-schedules");

    // The two whole-workspace units, each collapsed from its own workspace.
    assert_eq!(
        ids(&scheduled),
        vec![
            "rust@contrib#test".to_string(),
            "rust@core#test".to_string()
        ]
    );

    // The mutual gating edges inside the co-scheduled cycle are stripped: the two
    // whole-workspace peers launch concurrently in one wave, so a surviving
    // intra-cycle gate would let a failing peer contradict an in-flight peer in
    // APPLY. Each is left gated on nothing (the only edges were intra-cycle).
    let unit = |id: &str| {
        scheduled
            .units
            .iter()
            .find(|unit| unit.id == id)
            .unwrap_or_else(|| panic!("missing unit '{id}': {:?}", ids(&scheduled)))
    };
    assert!(unit("rust@core#test").depends_on.is_empty());
    assert!(unit("rust@contrib#test").depends_on.is_empty());

    // The strongly-connected component collapses into a single co-scheduled wave.
    assert_eq!(scheduled.waves.len(), 1);
    let mut wave = scheduled.waves[0].clone();
    wave.sort_unstable();
    assert_eq!(
        wave,
        vec![
            "rust@contrib#test".to_string(),
            "rust@core#test".to_string()
        ]
    );
}

#[test]
fn whole_workspace_facade_cycle_without_closure_capability_fails_closed() {
    // The identical whole-workspace facade cycle, but the adapter's task does NOT
    // carry the verified `workspace_closure` capability — the shape of an arbitrary
    // custom whole-workspace command that could hand output to another workspace.
    // Fan-out alone is not proof of self-containment, so the leveler must keep the
    // cycle failing closed rather than strip its real edges and co-schedule it.
    let federation = Federation {
        workspaces: vec![workspace("core"), workspace("contrib")],
        modules: vec![
            module("rust", "base", "core"),
            module("rust", "suite", "core"),
            module("rust", "plugin", "contrib"),
        ],
        edges: vec![
            Edge::new(
                mref("rust", "plugin"),
                mref("rust", "base"),
                DepKind::Normal,
            ),
            Edge::new(
                mref("rust", "suite"),
                mref("rust", "plugin"),
                DepKind::Normal,
            ),
        ],
        warnings: Vec::new(),
    };
    let mut adapters = ConfiguredSet::new();
    adapters.insert(
        eid("rust"),
        adapter_with_closure(
            "rust",
            RunStrategy::LeafToTop,
            FanOut::WholeWorkspace,
            false,
        ),
    );
    let active: Vec<toven_model::ModuleKey> = federation.modules.iter().map(Module::key).collect();
    let error = schedule(
        &request(),
        &federation,
        &active,
        &single_member(adapters),
        &GroupOverrides::default(),
        &toolchains(&federation),
    )
    .expect_err("a whole-workspace cycle without the closure capability must fail closed");
    assert!(
        error.to_string().contains("cannot be co-scheduled"),
        "{error}"
    );
}

#[test]
fn group_task_override_workspace_closure_true_and_false() {
    let federation = Federation {
        workspaces: vec![workspace("core"), workspace("contrib")],
        modules: vec![
            module("rust", "base", "core"),
            module("rust", "suite", "core"),
            module("rust", "plugin", "contrib"),
        ],
        edges: vec![
            Edge::new(
                mref("rust", "plugin"),
                mref("rust", "base"),
                DepKind::Normal,
            ),
            Edge::new(
                mref("rust", "suite"),
                mref("rust", "plugin"),
                DepKind::Normal,
            ),
        ],
        warnings: Vec::new(),
    };

    // 1. Adapter defaults workspace_closure=true, but group task override sets workspace_closure=false.
    // Must fail closed on the cycle.
    let mut adapters = ConfiguredSet::new();
    adapters.insert(
        eid("rust"),
        adapter_with_closure("rust", RunStrategy::LeafToTop, FanOut::WholeWorkspace, true),
    );
    let active: Vec<toven_model::ModuleKey> = federation.modules.iter().map(Module::key).collect();

    let mut overrides_false = GroupOverrides::default();
    let group_cfg_false = GroupConfig {
        tasks: std::collections::BTreeMap::from([(
            "test".to_string(),
            TaskOverride {
                workspace_closure: Some(false),
                ..TaskOverride::default()
            },
        )]),
        ..GroupConfig::default()
    };
    let members: std::collections::BTreeSet<ModuleKey> = active.iter().cloned().collect();
    overrides_false
        .record("override_false", &group_cfg_false, &members)
        .unwrap();

    let error = schedule(
        &request(),
        &federation,
        &active,
        &single_member(adapters),
        &overrides_false,
        &toolchains(&federation),
    )
    .expect_err("group override resetting workspace_closure to false must fail closed on cycle");
    assert!(
        error.to_string().contains("cannot be co-scheduled"),
        "{error}"
    );

    // 2. Adapter defaults workspace_closure=false, but group task override sets workspace_closure=true.
    // Must co-schedule the cycle.
    let mut adapters = ConfiguredSet::new();
    adapters.insert(
        eid("rust"),
        adapter_with_closure("rust", RunStrategy::LeafToTop, FanOut::WholeWorkspace, false),
    );
    let mut overrides_true = GroupOverrides::default();
    let group_cfg_true = GroupConfig {
        tasks: std::collections::BTreeMap::from([(
            "test".to_string(),
            TaskOverride {
                workspace_closure: Some(true),
                ..TaskOverride::default()
            },
        )]),
        ..GroupConfig::default()
    };
    overrides_true
        .record("override_true", &group_cfg_true, &members)
        .unwrap();

    let scheduled = schedule(
        &request(),
        &federation,
        &active,
        &single_member(adapters),
        &overrides_true,
        &toolchains(&federation),
    )
    .expect("group override enabling workspace_closure to true co-schedules cycle");
    assert_eq!(scheduled.waves.len(), 1);
}

#[test]
fn whole_workspace_acyclic_dependency_orders_units_across_waves() {
    // Two whole-workspace units with a one-way dependency (no facade cycle): the
    // `app` workspace depends on the `core` workspace. SCC condensation leaves both
    // as singletons, so leveling still orders the dependency first and the
    // dependent one wave later — the acyclic ordering guarantee is untouched.
    let federation = Federation {
        workspaces: vec![workspace("core"), workspace("app")],
        modules: vec![
            module("rust", "base", "core"),
            module("rust", "service", "app"),
        ],
        edges: vec![Edge::new(
            mref("rust", "service"),
            mref("rust", "base"),
            DepKind::Normal,
        )],
        warnings: Vec::new(),
    };
    let mut adapters = ConfiguredSet::new();
    adapters.insert(
        eid("rust"),
        adapter_with("rust", RunStrategy::LeafToTop, FanOut::WholeWorkspace),
    );
    let active: Vec<toven_model::ModuleKey> = federation.modules.iter().map(Module::key).collect();
    let scheduled = schedule(
        &request(),
        &federation,
        &active,
        &single_member(adapters),
        &GroupOverrides::default(),
        &toolchains(&federation),
    )
    .expect("an acyclic whole-workspace pair schedules");

    assert_eq!(
        scheduled.waves,
        vec![
            vec!["rust@core#test".to_string()],
            vec!["rust@app#test".to_string()],
        ]
    );
}

#[test]
fn named_extra_task_is_selected_by_its_addressable_name() {
    // A named extra (`test-integration`, kind = "test") is advertised by discovery
    // and suggestions; selection must resolve the user token to it by its
    // addressable name — the plain `test` token must still resolve the unnamed Test
    // task independently, with no collision either way.
    let federation = Federation {
        workspaces: vec![workspace("rust")],
        modules: vec![module("rust", "app", "rust")],
        edges: Vec::new(),
        warnings: Vec::new(),
    };
    let mut adapters = ConfiguredSet::new();
    adapters.insert(eid("rust"), named_extra_adapter("rust"));
    let adapters = single_member(adapters);
    let active = vec![toven_model::ModuleKey::bare(mref("rust", "app"))];
    let toolchains = toolchains(&federation);

    // The user token `test-integration` resolves the named extra's argv.
    let extra = schedule(
        &request_for(TaskIntent::resolve("test-integration")),
        &federation,
        &active,
        &adapters,
        &GroupOverrides::default(),
        &toolchains,
    )
    .expect("named extra schedules");
    assert_eq!(extra.units.len(), 1);
    assert_eq!(extra.units[0].argv, ["integration", "test"]);

    // The plain `test` token still resolves the unnamed Test task.
    let plain = schedule(
        &request_for(TaskIntent::resolve("test")),
        &federation,
        &active,
        &adapters,
        &GroupOverrides::default(),
        &toolchains,
    )
    .expect("plain test schedules");
    assert_eq!(plain.units.len(), 1);
    assert_eq!(plain.units[0].argv, ["plain", "test"]);
}

#[test]
fn group_override_applies_to_a_named_extra_by_its_addressable_name() {
    // A `[groups.*].tasks.test-integration` override is keyed by the extra's
    // addressable name; it must field-merge onto the resolved named extra.
    let federation = Federation {
        workspaces: vec![workspace("rust")],
        modules: vec![module("rust", "app", "rust")],
        edges: Vec::new(),
        warnings: Vec::new(),
    };
    let mut adapters = ConfiguredSet::new();
    adapters.insert(eid("rust"), named_extra_adapter("rust"));
    let group = crate::config::GroupConfig {
        tasks: BTreeMap::from([(
            "test-integration".to_string(),
            task_override(&["nextest", "run", "--test", "it"]),
        )]),
        ..crate::config::GroupConfig::default()
    };
    let overrides = group_overrides(
        "integration",
        &group,
        &[toven_model::ModuleKey::bare(mref("rust", "app"))],
    );
    let active = vec![toven_model::ModuleKey::bare(mref("rust", "app"))];
    let scheduled = schedule(
        &request_for(TaskIntent::resolve("test-integration")),
        &federation,
        &active,
        &single_member(adapters),
        &overrides,
        &toolchains(&federation),
    )
    .expect("named extra with group override schedules");
    assert_eq!(scheduled.units.len(), 1);
    assert_eq!(scheduled.units[0].argv, ["nextest", "run", "--test", "it"]);
    assert_eq!(scheduled.units[0].origin, TaskOrigin::Group);
}

fn task_override(argv: &[&str]) -> toven_ports::TaskOverride {
    toven_ports::TaskOverride {
        argv: Some(argv.iter().map(ToString::to_string).collect()),
        ..toven_ports::TaskOverride::default()
    }
}

fn group_overrides(
    name: &str,
    group: &crate::config::GroupConfig,
    members: &[toven_model::ModuleKey],
) -> GroupOverrides {
    let mut overrides = GroupOverrides::default();
    overrides
        .record(name, group, &members.iter().cloned().collect())
        .expect("group overrides record");
    overrides
}

#[test]
fn group_task_override_applies_to_members_only() {
    let federation = Federation {
        workspaces: vec![workspace("rust")],
        modules: vec![
            module("rust", "app", "rust"),
            module("rust", "errors", "rust"),
        ],
        edges: Vec::new(),
        warnings: Vec::new(),
    };
    let mut adapters = ConfiguredSet::new();
    adapters.insert(
        eid("rust"),
        adapter_with("rust", RunStrategy::Unordered, FanOut::Batchable),
    );
    let group = crate::config::GroupConfig {
        tasks: BTreeMap::from([("test".to_string(), task_override(&["nextest", "run"]))]),
        ..crate::config::GroupConfig::default()
    };
    let overrides = group_overrides(
        "integration",
        &group,
        &[toven_model::ModuleKey::bare(mref("rust", "app"))],
    );

    let active: Vec<toven_model::ModuleKey> = federation.modules.iter().map(Module::key).collect();
    let scheduled = schedule(
        &request(),
        &federation,
        &active,
        &single_member(adapters),
        &overrides,
        &toolchains(&federation),
    )
    .unwrap();

    // The overridden member splits into its own group-tagged unit; the non-member
    // keeps the ecosystem default in the plain batch unit.
    let overridden = scheduled
        .units
        .iter()
        .find(|unit| unit.id == "rust@rust~integration#test")
        .expect("group-tagged unit present");
    assert_eq!(overridden.argv, ["nextest", "run"]);
    assert_eq!(overridden.members, [mref("rust", "app").into()]);
    let default = scheduled
        .units
        .iter()
        .find(|unit| unit.id == "rust@rust#test")
        .expect("default unit present");
    assert_eq!(default.argv, ["x"]);
    assert_eq!(default.members, [mref("rust", "errors").into()]);
}

#[test]
fn same_name_group_overrides_from_distinct_scopes_do_not_collapse() {
    // Two modules in the same batch base, overridden by a member-local group and an
    // umbrella group that share the plain name `integration` but carry different
    // argv. Folding the plain name would collapse them into one
    // `…~integration#test` unit and render argv from the representative only; the
    // scope-qualified identity must keep them in distinct units.
    let federation = Federation {
        workspaces: vec![workspace("rust")],
        modules: vec![
            module("rust", "app", "rust"),
            module("rust", "errors", "rust"),
        ],
        edges: Vec::new(),
        warnings: Vec::new(),
    };
    let mut adapters = ConfiguredSet::new();
    adapters.insert(
        eid("rust"),
        adapter_with("rust", RunStrategy::Unordered, FanOut::Batchable),
    );

    let mut overrides = GroupOverrides::default();
    let local = crate::config::GroupConfig {
        tasks: BTreeMap::from([("test".to_string(), task_override(&["local", "run"]))]),
        ..crate::config::GroupConfig::default()
    };
    overrides
        .record(
            "member.billing.integration",
            &local,
            &std::iter::once(toven_model::ModuleKey::bare(mref("rust", "app"))).collect(),
        )
        .expect("member-local records");
    let umbrella = crate::config::GroupConfig {
        tasks: BTreeMap::from([("test".to_string(), task_override(&["umbrella", "run"]))]),
        ..crate::config::GroupConfig::default()
    };
    overrides
        .record(
            "umbrella.integration",
            &umbrella,
            &std::iter::once(toven_model::ModuleKey::bare(mref("rust", "errors"))).collect(),
        )
        .expect("umbrella records");

    let active: Vec<toven_model::ModuleKey> = federation.modules.iter().map(Module::key).collect();
    let scheduled = schedule(
        &request(),
        &federation,
        &active,
        &single_member(adapters),
        &overrides,
        &toolchains(&federation),
    )
    .unwrap();

    let member_local = scheduled
        .units
        .iter()
        .find(|unit| unit.id == "rust@rust~member.billing.integration#test")
        .expect("member-local unit present");
    assert_eq!(member_local.argv, ["local", "run"]);
    assert_eq!(member_local.members, [mref("rust", "app").into()]);
    let umbrella_unit = scheduled
        .units
        .iter()
        .find(|unit| unit.id == "rust@rust~umbrella.integration#test")
        .expect("umbrella unit present");
    assert_eq!(umbrella_unit.argv, ["umbrella", "run"]);
    assert_eq!(umbrella_unit.members, [mref("rust", "errors").into()]);
}

#[test]
fn group_run_strategy_override_relaxes_members_only() {
    let federation = Federation {
        workspaces: vec![workspace("rust")],
        modules: vec![
            module("rust", "app", "rust"),
            module("rust", "errors", "rust"),
        ],
        edges: vec![Edge::new(
            mref("rust", "app"),
            mref("rust", "errors"),
            DepKind::Normal,
        )],
        warnings: Vec::new(),
    };
    let mut adapters = ConfiguredSet::new();
    // Adapter default is dependency-respecting, so without an override the edge
    // orders `errors` before `app` across two waves.
    adapters.insert(eid("rust"), adapter("rust", RunStrategy::LeafToTop));
    let group = crate::config::GroupConfig {
        run_strategy: Some(RunStrategy::Unordered),
        ..crate::config::GroupConfig::default()
    };
    let overrides = group_overrides(
        "flat",
        &group,
        &[toven_model::ModuleKey::bare(mref("rust", "app"))],
    );

    let active: Vec<toven_model::ModuleKey> = federation.modules.iter().map(Module::key).collect();
    let waves = schedule(
        &request(),
        &federation,
        &active,
        &single_member(adapters),
        &overrides,
        &toolchains(&federation),
    )
    .unwrap()
    .waves;

    // The dependent's `unordered` override drops its intra-ecosystem edge, so both
    // modules collapse into a single wave.
    assert_eq!(waves.len(), 1);
}

/// An adapter whose sole task is a persistent `run` (`kind = "run"`), so a
/// schedule over a `run` intent proves the runnable filter.
fn run_adapter(ecosystem: &str) -> Box<dyn ConfiguredAdapter> {
    let mut task = Task::new("run", vec!["run".to_string()], FanOut::PerModule);
    task.persistent = true;
    Box::new(
        FakeConfiguredAdapter::new(eid(ecosystem))
            .with_response(DiscoverResponse::new(eid(ecosystem)))
            .with_tasks(vec![task])
            .with_run_strategy(RunStrategy::Unordered),
    )
}

#[test]
fn run_task_skips_library_only_modules() {
    let mut app = module("rust", "app", "rust");
    app.runnable = true;
    let mut lib = module("rust", "lib", "rust");
    lib.runnable = false;
    let federation = Federation {
        workspaces: vec![workspace("rust")],
        modules: vec![app, lib],
        edges: Vec::new(),
        warnings: Vec::new(),
    };
    let mut adapters = ConfiguredSet::new();
    adapters.insert(eid("rust"), run_adapter("rust"));
    let active: Vec<toven_model::ModuleKey> = federation.modules.iter().map(Module::key).collect();

    let scheduled = schedule(
        &request_for(TaskIntent::resolve("run")),
        &federation,
        &active,
        &single_member(adapters),
        &GroupOverrides::default(),
        &toolchains(&federation),
    )
    .unwrap();

    // Only the module with an executable target gets a `run` unit; the library-only
    // crate is dropped rather than scheduled to fail at exec.
    assert_eq!(ids(&scheduled), vec!["rust:app#run".to_string()]);
}

#[test]
fn non_run_tasks_never_filter_library_only_modules() {
    let mut app = module("rust", "app", "rust");
    app.runnable = true;
    let mut lib = module("rust", "lib", "rust");
    lib.runnable = false;
    let federation = Federation {
        workspaces: vec![workspace("rust")],
        modules: vec![app, lib],
        edges: Vec::new(),
        warnings: Vec::new(),
    };
    let mut adapters = ConfiguredSet::new();
    adapters.insert(eid("rust"), adapter("rust", RunStrategy::Unordered));

    // The default `test` task is not `run`-kind, so both modules keep a unit
    // regardless of `runnable`.
    assert_eq!(
        waves_for(&federation, &single_member(adapters)),
        vec![vec![
            "rust:app#test".to_string(),
            "rust:lib#test".to_string()
        ]]
    );
}

/// Build a bare [`PlannedUnit`] for direct leveling tests: only the id,
/// whole-workspace flag, and gating edges vary; every other field takes an
/// inert default.
fn planned_unit(
    id: &str,
    cycle_co_schedulable: bool,
    depends_on: &[&str],
) -> super::unit::PlannedUnit {
    super::unit::PlannedUnit {
        id: id.to_string(),
        module: toven_model::ModuleKey::bare(mref("rust", "m")),
        members: vec![toven_model::ModuleKey::bare(mref("rust", "m"))],
        task: "test".to_string(),
        origin: TaskOrigin::AdapterDefault,
        workspace: None,
        argv: Vec::new(),
        persistent: false,
        readiness: toven_model::ExecutionReadiness::Started,
        readiness_timeout: std::time::Duration::from_secs(0),
        base_argv: Vec::new(),
        shared_inputs: Vec::new(),
        cache_args: false,
        cacheable: true,
        fail_if_output: false,
        toolchain_identity: String::new(),
        depends_on: depends_on.iter().map(|dep| (*dep).to_string()).collect(),
        cycle_co_schedulable,
        resource_group: None,
    }
}

#[test]
fn non_whole_workspace_cycle_fails_closed_instead_of_co_scheduling() {
    // A residual cycle among units where at least one is not a whole-workspace
    // invocation (here a `PerModule`-shaped pair) encodes a real build handoff no
    // co-scheduling can honor, so leveling fails closed with a typed cycle error
    // rather than silently condensing it into one wave.
    let mut units = vec![
        planned_unit("rust:a#test", false, &["rust:b#test"]),
        planned_unit("rust:b#test", false, &["rust:a#test"]),
    ];
    let error = super::grouping::level_units_into_waves(&mut units)
        .expect_err("a non-whole-workspace cycle must fail closed");
    let message = error.to_string();
    assert!(message.contains("cannot be co-scheduled"), "{message}");
    assert!(message.contains("rust:a#test"), "{message}");
    assert!(message.contains("rust:b#test"), "{message}");
}

#[test]
fn mixed_cycle_with_one_non_whole_workspace_unit_fails_closed() {
    // Eligibility is all-or-nothing: a cycle of two whole-workspace units and one
    // non-whole-workspace unit is still rejected — the single real handoff makes
    // the whole component un-co-schedulable.
    let mut units = vec![
        planned_unit("rust@core#test", true, &["rust:leaf#test"]),
        planned_unit("rust@app#test", true, &["rust@core#test"]),
        planned_unit("rust:leaf#test", false, &["rust@app#test"]),
    ];
    let error = super::grouping::level_units_into_waves(&mut units)
        .expect_err("a mixed cycle must fail closed");
    assert!(
        error.to_string().contains("cannot be co-scheduled"),
        "{error}"
    );
}

#[test]
fn all_whole_workspace_cycle_co_schedules_and_strips_intra_cycle_edges() {
    // A cycle whose units are all whole-workspace co-schedules into one wave and
    // has its mutual intra-cycle gating edges stripped, while a real
    // cross-component handoff into the cycle is preserved.
    let mut units = vec![
        planned_unit(
            "rust@core#test",
            true,
            &["rust@contrib#test", "rust:dep#test"],
        ),
        planned_unit("rust@contrib#test", true, &["rust@core#test"]),
        planned_unit("rust:dep#test", false, &[]),
    ];
    let waves = super::grouping::level_units_into_waves(&mut units)
        .expect("an all-whole-workspace cycle co-schedules");

    let unit = |id: &str| units.iter().find(|unit| unit.id == id).expect("unit");
    // The intra-cycle edge core⇄contrib is stripped; the real dep→core handoff
    // survives.
    assert_eq!(unit("rust@core#test").depends_on, vec!["rust:dep#test"]);
    assert!(unit("rust@contrib#test").depends_on.is_empty());
    // `dep` levels first; the co-scheduled cycle shares the next wave.
    assert_eq!(waves.len(), 2);
    assert_eq!(waves[0], vec!["rust:dep#test"]);
    let mut cycle_wave = waves[1].clone();
    cycle_wave.sort_unstable();
    assert_eq!(
        cycle_wave,
        vec![
            "rust@contrib#test".to_string(),
            "rust@core#test".to_string()
        ]
    );
}
