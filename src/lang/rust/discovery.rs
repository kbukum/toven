//! Rust workspace discovery adapter.

use crate::{
    core::{
        AppResult, DISCOVERY_SCHEMA_VERSION, DiscoverRequest, DiscoverResponse, LangAdapter,
        validate_discovery_request_schema,
    },
    lang::rust::metadata::discover_modules,
};

const RUST_LANGUAGE: &str = "rust";

/// Rust adapter backed by `cargo metadata`.
#[derive(Debug, Default, Clone)]
pub struct RustAdapter;

impl RustAdapter {
    /// Create a Rust adapter.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl LangAdapter for RustAdapter {
    fn language(&self) -> &str {
        RUST_LANGUAGE
    }

    fn discover(&self, request: &DiscoverRequest) -> AppResult<DiscoverResponse> {
        validate_discovery_request_schema("discovery_request.schema_version", request)?;

        let modules = discover_modules(&request.workspace_root)?;
        Ok(DiscoverResponse {
            schema_version: DISCOVERY_SCHEMA_VERSION,
            modules,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::core::{DISCOVERY_SCHEMA_VERSION, DiscoverRequest, LangAdapter};

    use super::RustAdapter;

    #[test]
    fn discovers_fixture_workspace() {
        let root = rskit_testutil::test_workspace!("rust-discovery");
        let workspace_path = root.path().join("rust-workspace");
        rskit_fs::sync_io::tree::copy_tree(
            &root
                .fixture_path("rust-workspace")
                .expect("rust fixture path"),
            &workspace_path,
            rskit_fs::sync_io::tree::CopyTreeOptions::default(),
        )
        .expect("copy rust fixture");

        let response = RustAdapter::new()
            .discover(&DiscoverRequest {
                schema_version: DISCOVERY_SCHEMA_VERSION,
                workspace_root: workspace_path,
            })
            .expect("rust discovery succeeds");

        let names: Vec<_> = response
            .modules
            .iter()
            .map(|module| module.name.as_str())
            .collect();
        assert_eq!(names, ["fixture-app", "fixture-core", "fixture-test-util"]);

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
    fn rejects_request_schema_mismatch_before_metadata_discovery() {
        let error = RustAdapter::new()
            .discover(&DiscoverRequest {
                schema_version: 0,
                workspace_root: std::path::PathBuf::from("/path/that/should/not/be/read"),
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
