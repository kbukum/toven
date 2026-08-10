//! Behavioral discovery tests for the Rust adapter, driven by real `cargo
//! metadata` against fixture workspaces. Configs come from testkit fixtures —
//! no inline TOML.

use std::sync::Arc;

use toven_exec::ProcessToolRunner;
use toven_model::{AbsPath, DepKind, EcosystemId, ModuleRef};
use toven_ports::{ConfiguredAdapter, DiscoverRequest, Provider};
use toven_rust::RustProvider;
use toven_testkit::fixtures;

/// Build a configured Rust adapter from a fixture adapter config.
fn configure(adapter_config: &str) -> Box<dyn ConfiguredAdapter> {
    let raw_text = fixtures::ecosystem_string("rust", adapter_config).expect("adapter fixture");
    let raw = toven_testkit::raw_subtree(&raw_text).expect("valid adapter toml");
    RustProvider::new(Arc::new(ProcessToolRunner::new()))
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
    let root = fixtures::ecosystem("rust", workspace).expect("workspace fixture");
    let request = DiscoverRequest::new(AbsPath::new(root).expect("absolute root"));
    adapter.discover(&request)
}

fn module_ref(name: &str) -> ModuleRef {
    ModuleRef::new(EcosystemId::new("rust").unwrap(), name).unwrap()
}

#[test]
fn single_crate_yields_one_module_and_workspace() {
    let response = discover("adapter/single-manifest.toml", "workspaces/single-crate");

    assert_eq!(response.schema_version, 1);
    assert_eq!(response.modules.len(), 1);
    let module = &response.modules[0];
    assert_eq!(module.id, module_ref("solo"));
    assert_eq!(module.package.as_deref(), Some("solo"));
    assert_eq!(module.root.as_path().to_string_lossy(), ".");

    assert_eq!(response.workspaces.len(), 1);
    assert_eq!(response.workspaces[0].toolchain.tool, "cargo");
    assert!(response.workspaces[0].toolchain.version.is_none());
    assert!(response.edges.is_empty());
}

#[test]
fn basic_workspace_discovers_its_member() {
    let response = discover("adapter/single-manifest.toml", "workspaces/basic");

    let names: Vec<&str> = response
        .modules
        .iter()
        .map(|m| m.id.name.as_str())
        .collect();
    assert_eq!(names, ["errors"]);
    let errors = &response.modules[0];
    assert_eq!(errors.root.as_path().to_string_lossy(), "core/errors");
    assert_eq!(
        errors
            .manifest
            .as_ref()
            .unwrap()
            .as_path()
            .to_string_lossy(),
        "core/errors/Cargo.toml"
    );
}

#[test]
fn cross_workspace_path_dep_becomes_an_edge() {
    let response = discover("adapter/cross-manifests.toml", "workspaces/cross");

    let mut names: Vec<&str> = response
        .modules
        .iter()
        .map(|m| m.id.name.as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["cross-app", "cross-core"]);

    // Two standalone workspaces, one per manifest.
    assert_eq!(response.workspaces.len(), 2);

    assert_eq!(response.edges.len(), 1);
    let edge = &response.edges[0];
    assert_eq!(edge.from.module, module_ref("cross-app"));
    assert_eq!(edge.to.module, module_ref("cross-core"));
    assert_eq!(edge.kind, DepKind::Normal);

    // Each standalone manifest is its own workspace; the ids derive from the
    // repo-relative workspace root.
    let mut workspace_ids: Vec<&str> = response.workspaces.iter().map(|w| w.id.as_str()).collect();
    workspace_ids.sort_unstable();
    assert_eq!(workspace_ids, ["rust:app", "rust:core"]);
}

#[test]
fn discovery_rejects_a_manifest_escaping_the_project_root() {
    let error = discover_result("adapter/escaping-manifest.toml", "workspaces/single-crate")
        .expect_err("escaping manifest is rejected before cargo runs");
    assert!(
        error.to_string().contains("escapes the project root"),
        "{error}"
    );
}

#[test]
fn discovery_surfaces_a_cargo_metadata_failure() {
    let error = discover_result("adapter/single-manifest.toml", "workspaces/broken")
        .expect_err("malformed manifest makes cargo metadata fail");
    assert!(error.to_string().contains("cargo metadata"), "{error}");
}

#[test]
fn modules_carry_resource_group_and_workspaces_carry_blast_radius() {
    let response = discover("adapter/single-manifest.toml", "workspaces/single-crate");

    let module = &response.modules[0];
    assert_eq!(module.resource_group.as_deref(), Some("cargo:."));

    let workspace = &response.workspaces[0];
    assert_eq!(workspace.blast_radius, ["Cargo.lock"]);
}
