//! Discovery protocol shared by native and command adapters.

use std::path::PathBuf;

use crate::core::Module;

/// Request passed to a language adapter during discovery.
#[derive(Debug, Clone, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct DiscoverRequest {
    /// Workspace root for the project being inspected.
    pub workspace_root: PathBuf,
}

/// Normalized discovery response emitted by every adapter.
#[derive(Debug, Clone, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct DiscoverResponse {
    /// Discovery schema version.
    pub schema_version: u16,
    /// Modules discovered in the workspace.
    pub modules: Vec<Module>,
}
