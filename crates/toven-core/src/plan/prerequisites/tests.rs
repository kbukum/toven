//! Behavioral tests for task prerequisites: configure-time validation, the
//! (task, module) expansion, and cross-task unit linking with wave re-levelling.

use std::collections::BTreeSet;
use std::time::Duration;

use toven_model::{
    AbsPath, CacheVerdict, Capability, DepKind, EcosystemId, Edge, ExecutionReadiness,
    ExecutionUnit, Module, ModuleKey, ModuleRef, RepoPath,
};
use toven_ports::{
    DiscoverResponse, FanOut, RunStrategy, Task, TaskDependency, TaskIntent, TaskOrigin,
    TaskOverride,
};
use toven_testkit::FakeConfiguredAdapter;

use super::expand::{Expanded, Needs, Node};
use super::{Scope, expand, link, validate, validate_overrides};
use crate::config::GroupConfig;
use crate::plan::affected::ActiveModules;
use crate::plan::configure::{ConfiguredSet, MemberAdapters};
use crate::plan::discover::Federation;
use crate::plan::overrides::GroupOverrides;
use crate::plan::request::PlanRequest;

const ECOSYSTEM: &str = "command";

fn eid() -> EcosystemId {
    EcosystemId::new(ECOSYSTEM).unwrap()
}

fn key(name: &str) -> ModuleKey {
    ModuleKey::bare(ModuleRef::new(eid(), name).unwrap())
}

fn node(task: &str, module: &str) -> Node {
    (task.to_string(), key(module))
}

fn task(name: &str, depends_on: &[&str]) -> Task {
    let mut task = Task::new(name, vec!["echo".to_string()], FanOut::PerModule);
    task.depends_on = depends_on
        .iter()
        .map(|value| TaskDependency::parse(value).unwrap())
        .collect();
    task
}

fn adapters(tasks: Vec<Task>) -> MemberAdapters {
    let mut set = ConfiguredSet::new();
    set.insert(
        eid(),
        Box::new(
            FakeConfiguredAdapter::new(eid())
                .with_response(DiscoverResponse::new(eid()))
                .with_tasks(tasks)
                .with_run_strategy(RunStrategy::Unordered),
        ),
    );
    let mut adapters = MemberAdapters::default();
    adapters.insert(None, set);
    adapters
}

fn validation_error(tasks: Vec<Task>) -> String {
    validate(&adapters(tasks)).unwrap_err().to_string()
}

/// `app -> lib -> core`, every module rooted at its own name.
fn chain() -> Federation {
    let module = |name: &str| {
        Module::new(
            ModuleRef::new(eid(), name).unwrap(),
            RepoPath::new(name).unwrap(),
        )
    };
    Federation {
        workspaces: Vec::new(),
        modules: vec![module("app"), module("lib"), module("core")],
        edges: vec![
            Edge::new(key("app"), key("lib"), DepKind::Normal),
            Edge::new(key("lib"), key("core"), DepKind::Normal),
        ],
        global_inputs: Vec::new(),
        warnings: Vec::new(),
    }
}

fn request(name: &str) -> PlanRequest {
    PlanRequest::new(
        "r",
        "t",
        TaskIntent::resolve(name),
        AbsPath::new("/repo").unwrap(),
    )
    .with_passthrough(vec!["--verbose".to_string()])
}

fn active(modules: &[&str]) -> ActiveModules {
    ActiveModules {
        modules: modules.iter().map(|name| key(name)).collect(),
        full_activation: Vec::new(),
        named: BTreeSet::new(),
    }
}

fn expand_chain(tasks: Vec<Task>, requested: &[(&str, &[&str])]) -> Expanded {
    expand_chain_with(tasks, &GroupOverrides::default(), requested)
}

fn expand_chain_with(
    tasks: Vec<Task>,
    overrides: &GroupOverrides,
    requested: &[(&str, &[&str])],
) -> Expanded {
    let federation = chain();
    let adapters = adapters(tasks);
    let scope = Scope {
        federation: &federation,
        adapters: &adapters,
        overrides,
    };
    let requests = requested.iter().map(|(name, _)| request(name)).collect();
    let actives = requested
        .iter()
        .map(|(_, modules)| active(modules))
        .collect();
    expand(requests, actives, &scope).unwrap()
}

fn active_names(expanded: &Expanded, task: &str) -> Vec<String> {
    let index = expanded
        .requests
        .iter()
        .position(|request| request.intent.name() == task)
        .unwrap();
    expanded.actives[index]
        .modules
        .iter()
        .map(|module| module.module.name.clone())
        .collect()
}

fn unit(task: &str, module: &str) -> ExecutionUnit {
    ExecutionUnit {
        id: format!("{ECOSYSTEM}:{module}#{task}"),
        module: key(module),
        members: vec![key(module)],
        task: task.to_string(),
        origin: TaskOrigin::Project,
        workspace: None,
        argv: vec!["echo".to_string()],
        persistent: false,
        readiness: ExecutionReadiness::Started,
        readiness_timeout: Duration::from_secs(30),
        fail_if_output: false,
        cache: CacheVerdict::Disabled,
        cache_key: None,
        outputs: Vec::new(),
        depends_on: Vec::new(),
        resource_group: None,
    }
}

/// `((task, module), (prerequisite task, prerequisite module))`.
type NeedEdge<'a> = ((&'a str, &'a str), (&'a str, &'a str));

fn needs(edges: &[NeedEdge<'_>]) -> Needs {
    let mut needs = Needs::new();
    for ((task, module), (on_task, on_module)) in edges {
        needs
            .entry(node(task, module))
            .or_default()
            .insert(node(on_task, on_module));
    }
    needs
}

fn wave(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| format!("{ECOSYSTEM}:{id}")).collect()
}

#[test]
fn validate_accepts_a_dependency_scoped_self_reference() {
    validate(&adapters(vec![task("build", &["^build"])])).unwrap();
}

#[test]
fn validate_rejects_an_unknown_prerequisite_task() {
    let message = validation_error(vec![task("test", &["^compile"])]);
    assert!(message.contains("tasks.test.depends_on"), "{message}");
    assert!(message.contains("no 'compile' task"), "{message}");
}

#[test]
fn validate_rejects_a_persistent_prerequisite() {
    let mut serve = task("serve", &[]);
    serve.persistent = true;
    let message = validation_error(vec![serve, task("test", &["serve"])]);
    assert!(message.contains("'serve' is persistent"), "{message}");
}

#[test]
fn validate_allows_a_persistent_task_to_have_prerequisites() {
    let mut dev = task("dev", &["build"]);
    dev.persistent = true;
    validate(&adapters(vec![task("build", &[]), dev])).unwrap();
}

#[test]
fn validate_rejects_a_prerequisite_cycle() {
    let message = validation_error(vec![task("build", &["test"]), task("test", &["^build"])]);
    assert!(
        message.contains("cycle: build -> test -> build"),
        "{message}"
    );
}

#[test]
fn validate_rejects_outputs_outside_the_module() {
    for output in ["../dist", ".", "/abs"] {
        let mut build = task("build", &[]);
        build.outputs = vec![output.to_string()];
        let message = validation_error(vec![build]);
        assert!(
            message.contains("tasks.build.outputs"),
            "{output}: {message}"
        );
    }
}

#[test]
fn expand_adds_dependency_prerequisites_recursively_as_a_new_request() {
    let expanded = expand_chain(
        vec![task("build", &["^build"]), task("test", &["^build"])],
        &[("test", &["app"])],
    );
    let names: Vec<&str> = expanded
        .requests
        .iter()
        .map(|request| request.intent.name())
        .collect();
    assert_eq!(names, ["test", "build"]);
    assert_eq!(active_names(&expanded, "build"), ["core", "lib"]);
    // Passthrough args belong to the requested task only.
    assert_eq!(expanded.requests[0].passthrough, ["--verbose"]);
    assert!(expanded.requests[1].passthrough.is_empty());
    assert_eq!(
        expanded.needs,
        needs(&[
            (("test", "app"), ("build", "lib")),
            (("build", "lib"), ("build", "core")),
        ])
    );
}

#[test]
fn expand_adds_a_same_module_prerequisite() {
    let expanded = expand_chain(
        vec![task("build", &[]), task("test", &["build"])],
        &[("test", &["lib"])],
    );
    assert_eq!(active_names(&expanded, "build"), ["lib"]);
    assert_eq!(
        expanded.needs,
        needs(&[(("test", "lib"), ("build", "lib"))])
    );
}

#[test]
fn expand_merges_prerequisites_into_an_existing_request_for_the_task() {
    let expanded = expand_chain(
        vec![task("build", &[]), task("test", &["^build"])],
        &[("build", &["app"]), ("test", &["app"])],
    );
    assert_eq!(expanded.requests.len(), 2);
    assert_eq!(active_names(&expanded, "build"), ["app", "lib"]);
}

#[test]
fn expand_without_prerequisites_returns_the_requests_unchanged() {
    let expanded = expand_chain(
        vec![task("build", &[]), task("test", &[])],
        &[("test", &["app"])],
    );
    assert_eq!(expanded.requests.len(), 1);
    assert!(expanded.needs.is_empty());
}

#[test]
fn expand_walks_through_a_module_the_prerequisite_cannot_run_on() {
    let mut build = task("build", &["^build"]);
    build.requires = BTreeSet::from([Capability::new("bin").unwrap()]);
    let expanded = expand_chain(
        vec![build, task("test", &["^build"])],
        &[("test", &["app"])],
    );
    // `lib#build` is skipped at schedule time but still pulls in `core#build`.
    assert_eq!(active_names(&expanded, "build"), ["core", "lib"]);
    assert!(expanded.needs.contains_key(&node("build", "lib")));
}

/// A group `artifact` over `members` overriding `task`'s `depends_on`.
fn depends_on_override(task: &str, depends_on: &[&str], members: &[&str]) -> GroupOverrides {
    let over = TaskOverride {
        depends_on: Some(
            depends_on
                .iter()
                .map(|value| TaskDependency::parse(value).unwrap())
                .collect(),
        ),
        ..TaskOverride::default()
    };
    let group = GroupConfig {
        tasks: std::collections::BTreeMap::from([(task.to_string(), over)]),
        ..GroupConfig::default()
    };
    let mut overrides = GroupOverrides::default();
    overrides
        .record(
            "artifact",
            &group,
            &members.iter().map(|name| key(name)).collect(),
        )
        .unwrap();
    overrides
}

fn override_error(tasks: Vec<Task>, overrides: &GroupOverrides) -> String {
    validate_overrides(&adapters(tasks), &chain(), overrides)
        .unwrap_err()
        .to_string()
}

#[test]
fn expand_uses_a_group_override_depends_on_for_its_members_only() {
    let overrides = depends_on_override("test", &["^build", "build"], &["lib"]);
    let expanded = expand_chain_with(
        vec![task("build", &["^build"]), task("test", &["^build"])],
        &overrides,
        &[("test", &["app", "lib"])],
    );
    // `lib#test` also needs its own build; `app#test` keeps the default.
    assert_eq!(active_names(&expanded, "build"), ["core", "lib"]);
    assert_eq!(
        expanded.needs,
        needs(&[
            (("test", "app"), ("build", "lib")),
            (("test", "lib"), ("build", "core")),
            (("test", "lib"), ("build", "lib")),
            (("build", "lib"), ("build", "core")),
        ])
    );
}

#[test]
fn validate_overrides_accepts_a_known_prerequisite() {
    let overrides = depends_on_override("test", &["^build", "build"], &["lib"]);
    validate_overrides(
        &adapters(vec![task("build", &["^build"]), task("test", &["^build"])]),
        &chain(),
        &overrides,
    )
    .unwrap();
}

#[test]
fn validate_overrides_rejects_an_unknown_prerequisite() {
    let overrides = depends_on_override("test", &["compile"], &["lib"]);
    let message = override_error(vec![task("test", &[])], &overrides);
    assert!(
        message.contains("groups.artifact.tasks.test.depends_on"),
        "{message}"
    );
    assert!(message.contains("no 'compile' task"), "{message}");
}

#[test]
fn validate_overrides_rejects_a_persistent_prerequisite() {
    let mut serve = task("serve", &[]);
    serve.persistent = true;
    let overrides = depends_on_override("test", &["serve"], &["lib"]);
    let message = override_error(vec![serve, task("test", &[])], &overrides);
    assert!(message.contains("'serve' is persistent"), "{message}");
}

#[test]
fn validate_overrides_rejects_a_persistent_override_on_a_prerequisite() {
    let over = TaskOverride {
        persistent: Some(true),
        ..TaskOverride::default()
    };
    let group = GroupConfig {
        tasks: std::collections::BTreeMap::from([("build".to_string(), over)]),
        ..GroupConfig::default()
    };
    let mut overrides = GroupOverrides::default();
    overrides
        .record("artifact", &group, &BTreeSet::from([key("lib")]))
        .unwrap();
    let message = override_error(
        vec![task("build", &[]), task("test", &["build"])],
        &overrides,
    );
    assert!(
        message.contains("groups.artifact.tasks.build.persistent"),
        "{message}"
    );
    assert!(
        message.contains("task 'test' on 'command:lib' depends on 'build'"),
        "{message}"
    );
}

#[test]
fn validate_overrides_rejects_a_persistent_override_a_dependent_waits_on() {
    let over = TaskOverride {
        persistent: Some(true),
        ..TaskOverride::default()
    };
    let group = GroupConfig {
        tasks: std::collections::BTreeMap::from([("build".to_string(), over)]),
        ..GroupConfig::default()
    };
    let mut overrides = GroupOverrides::default();
    overrides
        .record("artifact", &group, &BTreeSet::from([key("lib")]))
        .unwrap();
    // `app#test` waits on `lib#build` through `^build`; `lib#test` only
    // reaches the non-persistent `core#build`.
    let message = override_error(
        vec![task("build", &[]), task("test", &["^build"])],
        &overrides,
    );
    assert!(
        message.contains("groups.artifact.tasks.build.persistent"),
        "{message}"
    );
    assert!(
        message.contains("task 'test' on 'command:app' depends on '^build'"),
        "{message}"
    );
    assert!(message.contains("persistent on 'command:lib'"), "{message}");
}

#[test]
fn validate_overrides_accepts_a_persistent_override_no_dependent_reaches() {
    let over = TaskOverride {
        persistent: Some(true),
        ..TaskOverride::default()
    };
    let group = GroupConfig {
        tasks: std::collections::BTreeMap::from([("build".to_string(), over)]),
        ..GroupConfig::default()
    };
    let mut overrides = GroupOverrides::default();
    overrides
        .record("artifact", &group, &BTreeSet::from([key("app")]))
        .unwrap();
    // Nothing depends on `app`, so its persistent build blocks no one.
    validate_overrides(
        &adapters(vec![task("build", &[]), task("test", &["^build"])]),
        &chain(),
        &overrides,
    )
    .unwrap();
}

#[test]
fn validate_overrides_accepts_a_persistent_override_the_group_routes_around() {
    let group = GroupConfig {
        tasks: std::collections::BTreeMap::from([
            (
                "build".to_string(),
                TaskOverride {
                    persistent: Some(true),
                    ..TaskOverride::default()
                },
            ),
            (
                "test".to_string(),
                TaskOverride {
                    depends_on: Some(Vec::new()),
                    ..TaskOverride::default()
                },
            ),
        ]),
        ..GroupConfig::default()
    };
    let mut overrides = GroupOverrides::default();
    overrides
        .record("artifact", &group, &BTreeSet::from([key("lib")]))
        .unwrap();
    validate_overrides(
        &adapters(vec![task("build", &[]), task("test", &["build"])]),
        &chain(),
        &overrides,
    )
    .unwrap();
}

#[test]
fn validate_overrides_rejects_outputs_outside_the_module() {
    let over = TaskOverride {
        outputs: Some(vec!["../dist".to_string()]),
        ..TaskOverride::default()
    };
    let group = GroupConfig {
        tasks: std::collections::BTreeMap::from([("build".to_string(), over)]),
        ..GroupConfig::default()
    };
    let mut overrides = GroupOverrides::default();
    overrides
        .record("artifact", &group, &BTreeSet::from([key("lib")]))
        .unwrap();
    let message = override_error(vec![task("build", &[])], &overrides);
    assert!(
        message.contains("groups.artifact.tasks.build.outputs"),
        "{message}"
    );
}

#[test]
fn validate_overrides_rejects_a_cycle_through_the_ecosystem_table() {
    let overrides = depends_on_override("build", &["test"], &["lib"]);
    let message = override_error(
        vec![task("build", &[]), task("test", &["build"])],
        &overrides,
    );
    assert!(
        message.contains("cycle: build -> test -> build"),
        "{message}"
    );
    assert!(
        message.contains("groups.artifact.tasks.build.depends_on"),
        "{message}"
    );
}

#[test]
fn link_orders_prerequisites_into_earlier_waves() {
    let mut units = vec![
        unit("test", "app"),
        unit("build", "lib"),
        unit("build", "core"),
    ];
    let waves = vec![wave(&["app#test"]), wave(&["core#build", "lib#build"])];
    let edges = needs(&[
        (("test", "app"), ("build", "lib")),
        (("build", "lib"), ("build", "core")),
    ]);
    let waves = link(&mut units, waves, &edges).unwrap();
    assert_eq!(
        waves,
        [
            wave(&["core#build"]),
            wave(&["lib#build"]),
            wave(&["app#test"])
        ]
    );
    assert_eq!(units[0].depends_on, wave(&["lib#build"]));
    assert_eq!(units[1].depends_on, wave(&["core#build"]));
}

#[test]
fn link_walks_through_a_prerequisite_without_a_unit() {
    let mut units = vec![unit("test", "app"), unit("build", "core")];
    let waves = vec![wave(&["app#test", "core#build"])];
    let edges = needs(&[
        (("test", "app"), ("build", "lib")),
        (("build", "lib"), ("build", "core")),
    ]);
    let waves = link(&mut units, waves, &edges).unwrap();
    assert_eq!(waves, [wave(&["core#build"]), wave(&["app#test"])]);
    assert_eq!(units[0].depends_on, wave(&["core#build"]));
}

#[test]
fn link_without_prerequisites_keeps_the_waves() {
    let mut units = vec![unit("test", "app")];
    let waves = vec![wave(&["app#test"])];
    assert_eq!(
        link(&mut units, waves.clone(), &Needs::new()).unwrap(),
        waves
    );
}

#[test]
fn link_rejects_prerequisites_that_close_a_unit_cycle() {
    let mut units = vec![unit("build", "app"), unit("build", "lib")];
    let waves = vec![wave(&["app#build", "lib#build"])];
    let edges = needs(&[
        (("build", "app"), ("build", "lib")),
        (("build", "lib"), ("build", "app")),
    ]);
    let error = link(&mut units, waves, &edges).unwrap_err();
    assert!(error.to_string().contains("cycle"), "{error}");
}
