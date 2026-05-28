//! Discovery protocol shared by native and command adapters.

#![allow(clippy::redundant_pub_crate)]

use std::path::PathBuf;

use crate::core::{AppError, AppResult, Module};

/// Current discovery protocol schema version.
pub const DISCOVERY_SCHEMA_VERSION: u16 = 1;

/// Request passed to a language adapter during discovery.
#[derive(Debug, Clone, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct DiscoverRequest {
    /// Discovery schema version.
    pub schema_version: u16,
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

pub(crate) fn validate_discovery_request_schema(
    field: impl AsRef<str>,
    request: &DiscoverRequest,
) -> AppResult<()> {
    if request.schema_version == DISCOVERY_SCHEMA_VERSION {
        return Ok(());
    }

    Err(AppError::invalid_input(
        field.as_ref(),
        format!(
            "unsupported discovery request schema {}",
            request.schema_version
        ),
    ))
}
