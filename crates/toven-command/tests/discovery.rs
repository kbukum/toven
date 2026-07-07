//! Behavioral discovery tests for the command adapter. The escape hatch shells
//! out to nothing — discovery just normalizes the declared module/edge set — so
//! these run against a synthetic project root. Configs come from testkit
//! fixture files.

use rskit_config::RawValue;
use toven_command::CommandProvider;
use toven_model::{AbsPath, DepKind, EcosystemId, ModuleRef};
use toven_ports::{ConfiguredAdapter, DiscoverRequest, DiscoverResponse, Provider};

/// Parse an adapter TOML subtree into a canonical raw config.
fn raw_subtree(toml: &str) -> RawValue {
    rskit_codec::decode(&rskit_codec::TomlCodec, toml).expect("raw subtree")
}

/// Resolve a command fixture path.
fn fixture(rel: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../toven-testkit/fixtures/ecosystems/command")
        .join(rel)
}

/// Read a command fixture file.
fn fixture_string(rel: &str) -> String {
    std::fs::read_to_string(fixture(rel)).expect("fixture")
}

/// Build a configured command adapter from a fixture adapter config.
fn configure(adapter_config: &str) -> Box<dyn ConfiguredAdapter> {
    let raw_text = fixture_string(adapter_config);
    let raw = raw_subtree(&raw_text);
    CommandProvider::new()
        .expect("provider")
        .configure(raw)
        .expect("configure")
}

/// Discover with a synthetic project root (never touched on disk).
fn discover_result(adapter_config: &str) -> rskit_errors::AppResult<DiscoverResponse> {
    let adapter = configure(adapter_config);
    let request = DiscoverRequest::new(AbsPath::new("/repo").expect("absolute root"));
    adapter.discover(&request)
}

fn module_ref(name: &str) -> ModuleRef {
    ModuleRef::new(EcosystemId::new("command").unwrap(), name).unwrap()
}

#[test]
fn declared_modules_become_a_single_command_workspace() {
    let response = discover_result("adapter/declared-modules.toml").expect("discover");

    assert_eq!(response.schema_version, 1);
    assert_eq!(response.modules.len(), 2);
    assert_eq!(response.workspaces.len(), 1);

    let workspace = &response.workspaces[0];
    assert_eq!(workspace.id.as_str(), "command");
    assert_eq!(workspace.root.as_path().to_string_lossy(), ".");
    assert_eq!(workspace.toolchain.tool, "command");
}

#[test]
fn module_metadata_and_manifest_round_trip() {
    let response = discover_result("adapter/declared-modules.toml").expect("discover");

    let api = response
        .modules
        .iter()
        .find(|m| m.id == module_ref("api"))
        .expect("api module");
    assert_eq!(api.root.as_path().to_string_lossy(), "services/api");
    assert_eq!(
        api.manifest
            .as_ref()
            .map(|p| p.as_path().to_string_lossy().into_owned()),
        Some("services/api/Makefile".to_string())
    );
    assert_eq!(api.resource_group.as_deref(), Some("command:api"));
    assert_eq!(
        api.workspace.as_ref().map(toven_model::WorkspaceId::as_str),
        Some("command")
    );
}

#[test]
fn depends_on_yields_a_normal_edge() {
    let response = discover_result("adapter/declared-modules.toml").expect("discover");

    assert_eq!(response.edges.len(), 1);
    let edge = &response.edges[0];
    assert_eq!(edge.from.module, module_ref("site"));
    assert_eq!(edge.to.module, module_ref("api"));
    assert_eq!(edge.kind, DepKind::Normal);
}

#[test]
fn depends_on_unknown_module_is_rejected() {
    let error = discover_result("adapter/unknown-dependency.toml").expect_err("rejected");
    assert!(error.to_string().contains("ghost"), "{error}");
}

#[test]
fn workspace_carries_no_blast_radius() {
    let response = discover_result("adapter/declared-modules.toml").expect("discover");
    // The escape hatch infers no shared inputs across declared commands.
    assert!(response.workspaces[0].blast_radius.is_empty());
}
