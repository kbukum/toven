//! Behavioral discovery tests for the Go adapter, driven by real `go mod edit`
//! against fixture workspaces. Configs come from testkit fixtures — no inline
//! TOML.

use rskit_config::RawValue;
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
    GoProvider::new()
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

    // A go.work grouping keys its blast radius off the workspace-level
    // go.work / go.work.sum, not a (nonexistent) root go.sum.
    let globs = &response.workspaces[0].blast_radius;
    assert_eq!(globs, &["go.work", "go.work.sum"]);
}

#[test]
fn versioned_modules_keep_distinct_names_instead_of_collapsing_onto_v_major() {
    let response = discover("adapter/versioned-modules.toml", "workspaces/versioned");

    let mut names: Vec<&str> = response
        .modules
        .iter()
        .map(|m| m.id.name.as_str())
        .collect();
    names.sort_unstable();
    // Without stripping the `/v2` suffix both modules would be named `v2` and
    // discovery would abort with a false duplicate-module conflict.
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
fn discovery_rejects_two_modules_resolving_to_the_same_name() {
    let error = discover_result("adapter/duplicate-name.toml", "workspaces/duplicate")
        .expect_err("two modules with the same final segment conflict");
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
fn modules_carry_resource_group_and_workspaces_carry_blast_radius() {
    let response = discover("adapter/single-module.toml", "workspaces/single-module");

    let module = &response.modules[0];
    assert_eq!(module.resource_group.as_deref(), Some("go:."));

    let workspace = &response.workspaces[0];
    assert_eq!(workspace.blast_radius, ["go.sum"]);
}
