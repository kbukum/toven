//! Behavioral discovery tests for the Go adapter, driven by real `go mod edit`
//! against fixture workspaces. Configs come from testkit fixtures — no inline
//! TOML.

use rskit_config::RawValue;
use std::sync::Arc;
use toven_exec::ProcessToolRunner;
use toven_go::GoProvider;
use toven_model::{AbsPath, DepKind, EcosystemId, ModuleRef};
use toven_ports::{ConfiguredAdapter, DiscoverRequest, Provider};

/// Parse an adapter TOML subtree into a canonical raw config.
fn raw_subtree(toml: &str) -> RawValue {
    rskit_codec::decode(&rskit_codec::TomlCodec, toml).expect("raw subtree")
}

/// Resolve a Go fixture path.
fn fixture(rel: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../toven-testkit/fixtures/ecosystems/go")
        .join(rel)
}

/// Read a Go fixture file.
fn fixture_string(rel: &str) -> String {
    std::fs::read_to_string(fixture(rel)).expect("fixture")
}

/// Build a configured Go adapter from a fixture adapter config.
fn configure(adapter_config: &str) -> Box<dyn ConfiguredAdapter> {
    let raw_text = fixture_string(adapter_config);
    let raw = raw_subtree(&raw_text);
    GoProvider::new(Arc::new(ProcessToolRunner::new()))
        .expect("provider")
        .configure(raw)
        .expect("configure")
}

/// Discover under a fixture workspace directory.
fn discover(adapter_config: &str, workspace: &str) -> toven_ports::DiscoverResponse {
    discover_result(adapter_config, workspace).expect("discover")
}

/// Discover under a fixture workspace directory, surfacing the typed error.
fn discover_result(
    adapter_config: &str,
    workspace: &str,
) -> rskit_errors::AppResult<toven_ports::DiscoverResponse> {
    let adapter = configure(adapter_config);
    let root = fixture(workspace);
    let request = DiscoverRequest::new(AbsPath::new(root).expect("absolute root"));
    adapter.discover(&request)
}

fn module_ref(name: &str) -> ModuleRef {
    ModuleRef::new(EcosystemId::new("go").unwrap(), name).unwrap()
}

#[test]
fn single_module_yields_one_module_and_workspace() {
    let response = discover("adapter/single-module.toml", "workspaces/single-module");

    assert_eq!(response.schema_version, 1);
    assert_eq!(response.modules.len(), 1);
    let module = &response.modules[0];
    assert_eq!(module.id, module_ref("solo"));
    assert_eq!(module.package.as_deref(), Some("example.com/solo"));
    assert_eq!(module.root.as_path().to_string_lossy(), ".");
    assert_eq!(
        module
            .manifest
            .as_ref()
            .unwrap()
            .as_path()
            .to_string_lossy(),
        "go.mod"
    );

    assert_eq!(response.workspaces.len(), 1);
    assert_eq!(response.workspaces[0].id.as_str(), "go");
    assert_eq!(response.workspaces[0].toolchain.tool, "go");
    assert!(response.workspaces[0].toolchain.version.is_none());
    assert!(response.edges.is_empty());
}

#[test]
fn go_work_groups_members_into_one_workspace_with_an_edge() {
    let response = discover("adapter/work-modules.toml", "workspaces/work");

    let mut names: Vec<&str> = response
        .modules
        .iter()
        .map(|m| m.id.name.as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["app", "core"]);

    // A single go.work workspace groups both members.
    assert_eq!(response.workspaces.len(), 1);
    assert_eq!(response.workspaces[0].id.as_str(), "go");
    assert_eq!(response.workspaces[0].root.as_path().to_string_lossy(), ".");
    for module in &response.modules {
        assert_eq!(module.workspace.as_ref().unwrap().as_str(), "go");
    }

    assert_eq!(response.edges.len(), 1);
    let edge = &response.edges[0];
    assert_eq!(edge.from.module, module_ref("app"));
    assert_eq!(edge.to.module, module_ref("core"));
    assert_eq!(edge.kind, DepKind::Normal);

    // A go.work grouping keys its blast radius off the workspace-level go.work /
    // go.work.sum, not a (nonexistent) root go.sum.
    let globs = &response.workspaces[0].blast_radius;
    assert_eq!(globs, &["go.work", "go.work.sum"]);
}

#[test]
fn nested_versioned_modules_are_named_by_directory_and_edges_resolve() {
    let response = discover("adapter/versioned-modules.toml", "workspaces/versioned");

    let mut names: Vec<&str> = response
        .modules
        .iter()
        .map(|m| m.id.name.as_str())
        .collect();
    names.sort_unstable();
    // Nested modules take their identity from the repo-relative directory, so two
    // `/v2` modules stay distinct (`alpha`, `beta`) with no false collision.
    assert_eq!(names, ["alpha", "beta"]);

    let mut packages: Vec<&str> = response
        .modules
        .iter()
        .filter_map(|m| m.package.as_deref())
        .collect();
    packages.sort_unstable();
    assert_eq!(packages, ["example.com/alpha/v2", "example.com/beta/v2"]);

    // Edges are keyed on the full module path, so the versioned require resolves.
    assert_eq!(response.edges.len(), 1);
    let edge = &response.edges[0];
    assert_eq!(edge.from.module, module_ref("alpha"));
    assert_eq!(edge.to.module, module_ref("beta"));
    assert_eq!(edge.kind, DepKind::Normal);
}

#[test]
fn auto_enumerates_go_work_members_without_a_hand_listed_set() {
    let response = discover("adapter/auto-modules.toml", "workspaces/work");

    let mut names: Vec<&str> = response
        .modules
        .iter()
        .map(|m| m.id.name.as_str())
        .collect();
    names.sort_unstable();
    // `modules = "auto"` derives the same members the explicit list names.
    assert_eq!(names, ["app", "core"]);

    assert_eq!(response.workspaces.len(), 1);
    assert_eq!(response.workspaces[0].id.as_str(), "go");
    assert_eq!(response.edges.len(), 1);
    assert_eq!(response.edges[0].from.module, module_ref("app"));
    assert_eq!(response.edges[0].to.module, module_ref("core"));
}

#[test]
fn discovery_rejects_two_modules_whose_directories_fold_to_the_same_name() {
    let error = discover_result("adapter/duplicate-name.toml", "workspaces/duplicate")
        .expect_err("directories `svc/api` and `svc-api` fold to the same name");
    assert!(error.to_string().contains("duplicate module"), "{error}");
}

#[test]
fn discovery_rejects_a_module_escaping_the_project_root() {
    let error = discover_result("adapter/escaping-module.toml", "workspaces/single-module")
        .expect_err("escaping module is rejected before go runs");
    assert!(
        error.to_string().contains("escapes the project root"),
        "{error}"
    );
}

#[test]
fn discovery_surfaces_a_go_mod_edit_failure() {
    let error = discover_result("adapter/single-module.toml", "workspaces/broken")
        .expect_err("malformed go.mod makes go mod edit fail");
    assert!(error.to_string().contains("go mod edit"), "{error}");
}

#[test]
fn modules_run_in_parallel_and_workspaces_carry_blast_radius() {
    let response = discover("adapter/single-module.toml", "workspaces/single-module");

    let module = &response.modules[0];
    assert_eq!(module.resource_group, None);

    let workspace = &response.workspaces[0];
    assert_eq!(workspace.blast_radius, ["go.sum"]);
}

#[test]
fn discovery_leaves_the_go_toolchain_to_the_user_environment() {
    let runner = Arc::new(
        toven_testkit::doubles::FakeToolRunner::new()
            .with_stdout(r#"{"Module":{"Path":"example.com/solo"},"Go":"1.27"}"#),
    );
    let raw = raw_subtree(&fixture_string("adapter/single-module.toml"));
    let adapter = GoProvider::new(runner.clone())
        .expect("provider")
        .configure(raw)
        .expect("configure");
    let root = fixture("workspaces/single-module");
    let request = DiscoverRequest::new(AbsPath::new(root).expect("absolute root"));

    adapter.discover(&request).expect("discover");

    let requests = runner.requests();
    assert!(!requests.is_empty(), "discovery ran go");
    for invocation in requests {
        assert!(
            !invocation.environment.vars.contains_key("GOTOOLCHAIN"),
            "{:?}",
            invocation.argv
        );
    }
}

#[test]
fn go_mod_edit_runs_in_the_manifest_directory() {
    // Go picks its toolchain (`GOTOOLCHAIN=auto`/`path`) from the `go.work` or
    // `go.mod` enclosing its working directory, so each nested module is read
    // from its own directory rather than the project root.
    let module = || {
        toven_ports::ToolOutcome::new(
            Some(0),
            r#"{"Module":{"Path":"example.com/any/v2"},"Go":"1.27"}"#,
            "",
        )
    };
    let runner = Arc::new(
        toven_testkit::doubles::FakeToolRunner::new().with_outcomes([
            toven_ports::ToolOutcome::new(
                Some(0),
                r#"{"Use":[{"DiskPath":"./alpha"},{"DiskPath":"./beta"}]}"#,
                "",
            ),
            module(),
            module(),
        ]),
    );
    let raw = raw_subtree(&fixture_string("adapter/versioned-modules.toml"));
    let adapter = GoProvider::new(runner.clone())
        .expect("provider")
        .configure(raw)
        .expect("configure");
    let root = fixture("workspaces/versioned");
    let request = DiscoverRequest::new(AbsPath::new(root.clone()).expect("absolute root"));

    adapter.discover(&request).expect("discover");

    let edits: Vec<_> = runner
        .requests()
        .into_iter()
        .filter(|invocation| invocation.argv.get(1).map(String::as_str) == Some("mod"))
        .collect();
    assert_eq!(edits.len(), 2, "one go mod edit per module");
    for invocation in edits {
        let manifest = std::path::PathBuf::from(invocation.argv.last().expect("manifest arg"));
        assert_ne!(manifest.parent(), Some(root.as_path()), "nested manifest");
        assert_eq!(
            invocation.working_dir(),
            manifest.parent(),
            "{:?}",
            invocation.argv
        );
    }
}
