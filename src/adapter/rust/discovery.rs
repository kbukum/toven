//! Rust workspace discovery adapter.

use crate::{
    adapter::rust::{RustProfileOptions, cargo::metadata::discover_modules, tasks},
    core::{
        AdapterId, AppResult, DISCOVERY_SCHEMA_VERSION, DiscoverRequest, DiscoverResponse,
        DiscoveryAdapter, Task, ToolchainProbe, validate_discovery_request_schema,
    },
};

const RUST_ADAPTER: &str = "rust";

/// Rust adapter backed by `cargo metadata`.
#[derive(Debug, Clone)]
pub struct RustAdapter {
    adapter_id: AdapterId,
}

impl RustAdapter {
    /// Create a Rust adapter.
    #[must_use]
    pub fn new() -> Self {
        Self {
            adapter_id: AdapterId::new(RUST_ADAPTER).expect("built-in adapter id is valid"),
        }
    }
}

impl Default for RustAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl DiscoveryAdapter for RustAdapter {
    fn adapter_id(&self) -> &AdapterId {
        &self.adapter_id
    }

    fn discover(&self, request: &DiscoverRequest) -> AppResult<DiscoverResponse> {
        validate_discovery_request_schema("discovery_request.schema_version", request)?;

        let options = RustProfileOptions::from_adapter_options(&request.adapter_options)?;
        let modules = discover_modules(&request.project_root, &options.manifests)?;
        Ok(DiscoverResponse {
            schema_version: DISCOVERY_SCHEMA_VERSION,
            scope_id: request.scope_id.clone(),
            adapter_id: request.adapter_id.clone(),
            modules: modules
                .into_iter()
                .map(|module| {
                    crate::core::DiscoveredModule::from_module(
                        module,
                        request.scope_id.clone(),
                        request.adapter_id.clone(),
                    )
                })
                .collect(),
        })
    }

    fn default_tasks(&self) -> Vec<Task> {
        tasks::default_tasks(&self.adapter_id)
    }

    fn toolchain_probes(&self) -> Vec<ToolchainProbe> {
        vec![
            ToolchainProbe {
                label: "cargo".to_string(),
                program: "cargo".to_string(),
                args: vec!["--version".to_string()],
            },
            ToolchainProbe {
                label: "rustc".to_string(),
                program: "rustc".to_string(),
                args: vec!["--version".to_string()],
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::core::{
        AdapterId, AdapterOptions, DISCOVERY_SCHEMA_VERSION, DiscoverRequest, DiscoveryAdapter,
        ScopeId,
    };

    use super::{RustAdapter, RustProfileOptions};

    #[test]
    fn discovers_fixture_workspace() {
        let root = rskit_testutil::test_workspace!("rust-discovery");
        let workspace_path = root.path().join("rust-workspace");
        rskit_fs::sync_io::tree::copy_tree(
            &root
                .fixture_path("rust/workspace")
                .expect("rust fixture path"),
            &workspace_path,
            rskit_fs::sync_io::tree::CopyTreeOptions::default(),
        )
        .expect("copy rust fixture");

        let response = RustAdapter::new()
            .discover(&DiscoverRequest {
                schema_version: DISCOVERY_SCHEMA_VERSION,
                project_root: workspace_path,
                scope_id: ScopeId::new("rust").expect("scope id"),
                adapter_id: AdapterId::new("rust").expect("adapter id"),
                scope_root: PathBuf::from("."),
                adapter_options: RustProfileOptions::from_manifests(vec![PathBuf::from(
                    "Cargo.toml",
                )])
                .expect("rust options")
                .to_adapter_options()
                .expect("adapter options"),
            })
            .expect("rust discovery succeeds");

        let names: Vec<_> = response
            .modules
            .iter()
            .map(|module| module.name.as_str())
            .collect();
        assert_eq!(names, ["fixture-app", "fixture-core", "fixture-test-util"]);
        assert_eq!(response.scope_id.as_str(), "rust");
        assert_eq!(response.adapter_id.as_str(), "rust");

        let app = response
            .modules
            .iter()
            .find(|module| module.name.as_str() == "fixture-app")
            .expect("app module exists");
        assert_eq!(app.root, std::path::PathBuf::from("crates/app"));
        assert_eq!(
            app.dependencies
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["fixture-core"]
        );
        assert!(
            !app.dependencies
                .iter()
                .any(|dependency| { dependency.as_str() == "fixture-test-util" })
        );
    }

    #[test]
    fn prefixes_modules_with_manifest_parent() {
        let root = rskit_testutil::test_workspace!("rust-profile-discovery");
        let workspace_path = root.path().join("project");
        rskit_fs::sync_io::tree::copy_tree(
            &root
                .fixture_path("rust/workspace")
                .expect("rust fixture path"),
            &workspace_path.join("core"),
            rskit_fs::sync_io::tree::CopyTreeOptions::default(),
        )
        .expect("copy rust fixture");

        let response = RustAdapter::new()
            .discover(&DiscoverRequest {
                schema_version: DISCOVERY_SCHEMA_VERSION,
                project_root: workspace_path,
                scope_id: ScopeId::new("rust").expect("scope id"),
                adapter_id: AdapterId::new("rust").expect("adapter id"),
                scope_root: PathBuf::from("."),
                adapter_options: RustProfileOptions::from_manifests(vec![PathBuf::from(
                    "core/Cargo.toml",
                )])
                .expect("rust options")
                .to_adapter_options()
                .expect("adapter options"),
            })
            .expect("rust discovery succeeds");

        let app = response
            .modules
            .iter()
            .find(|module| module.name.as_str() == "fixture-app")
            .expect("app module exists");
        assert_eq!(app.root, PathBuf::from("core/crates/app"));
        assert_eq!(app.manifest, Some(PathBuf::from("core/Cargo.toml")));
        assert_eq!(
            app.source_patterns,
            ["core/crates/app/Cargo.toml", "core/crates/app/src/**"]
        );
    }

    #[test]
    fn discovers_path_dependencies_across_configured_manifests() {
        let root = rskit_testutil::test_workspace!("rust-cross-workspace-discovery");
        let workspace_path = root.path().join("project");
        rskit_fs::sync_io::tree::copy_tree(
            &root
                .fixture_path("rust/cross-workspaces")
                .expect("rust fixture path"),
            &workspace_path,
            rskit_fs::sync_io::tree::CopyTreeOptions::default(),
        )
        .expect("copy rust fixture");

        let response = RustAdapter::new()
            .discover(&DiscoverRequest {
                schema_version: DISCOVERY_SCHEMA_VERSION,
                project_root: workspace_path,
                scope_id: ScopeId::new("rust").expect("scope id"),
                adapter_id: AdapterId::new("rust").expect("adapter id"),
                scope_root: PathBuf::from("."),
                adapter_options: RustProfileOptions::from_manifests(vec![
                    PathBuf::from("core/Cargo.toml"),
                    PathBuf::from("contrib/Cargo.toml"),
                ])
                .expect("rust options")
                .to_adapter_options()
                .expect("adapter options"),
            })
            .expect("rust discovery succeeds");

        let contrib = response
            .modules
            .iter()
            .find(|module| module.name.as_str() == "contrib-app")
            .expect("contrib module exists");
        assert_eq!(
            contrib
                .dependencies
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["core-local"]
        );
    }

    #[test]
    fn filters_path_dependencies_outside_configured_manifests() {
        let root = rskit_testutil::test_workspace!("rust-filter-external-path-deps");
        let workspace_path = root.path().join("project");
        rskit_fs::sync_io::tree::copy_tree(
            &root
                .fixture_path("rust/cross-workspaces")
                .expect("rust fixture path"),
            &workspace_path,
            rskit_fs::sync_io::tree::CopyTreeOptions::default(),
        )
        .expect("copy rust fixture");

        let response = RustAdapter::new()
            .discover(&DiscoverRequest {
                schema_version: DISCOVERY_SCHEMA_VERSION,
                project_root: workspace_path,
                scope_id: ScopeId::new("contrib").expect("scope id"),
                adapter_id: AdapterId::new("rust").expect("adapter id"),
                scope_root: PathBuf::from("."),
                adapter_options: RustProfileOptions::from_manifests(vec![PathBuf::from(
                    "contrib/Cargo.toml",
                )])
                .expect("rust options")
                .to_adapter_options()
                .expect("adapter options"),
            })
            .expect("rust discovery succeeds");

        let contrib = response
            .modules
            .iter()
            .find(|module| module.name.as_str() == "contrib-app")
            .expect("contrib module exists");
        assert!(contrib.dependencies.is_empty());
    }

    #[test]
    fn rejects_empty_manifest_paths() {
        let error = RustProfileOptions::from_manifests(vec![PathBuf::new()])
            .expect_err("empty manifest path should fail");

        assert!(error.message.contains("path cannot be empty"));
    }

    #[test]
    fn rejects_request_schema_mismatch_before_metadata_discovery() {
        let error = RustAdapter::new()
            .discover(&DiscoverRequest {
                schema_version: 0,
                project_root: std::path::PathBuf::from("/path/that/should/not/be/read"),
                scope_id: ScopeId::new("rust").expect("scope id"),
                adapter_id: AdapterId::new("rust").expect("adapter id"),
                scope_root: PathBuf::from("."),
                adapter_options: AdapterOptions::default(),
            })
            .expect_err("schema mismatch should fail before metadata discovery");

        assert!(error.message.contains("discovery_request.schema_version"));
        assert!(
            error
                .message
                .contains("unsupported discovery request schema")
        );
    }
}
