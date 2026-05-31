//! Discovery protocol shared by native and command adapters.

use std::{collections::BTreeMap, path::PathBuf};

use crate::core::{AdapterId, AppError, AppResult, Module, ModuleId, ScopeId};

/// Current discovery protocol schema version.
pub const DISCOVERY_SCHEMA_VERSION: u16 = 1;

/// Adapter-specific discovery options carried by the core protocol.
pub type AdapterOptions = BTreeMap<String, serde_json::Value>;

/// Request passed to an adapter during discovery.
#[derive(Debug, Clone, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct DiscoverRequest {
    /// Discovery schema version.
    pub schema_version: u16,
    /// Project root for git, cache, watch, and reporting.
    pub project_root: PathBuf,
    /// Scope being discovered.
    pub scope_id: ScopeId,
    /// Adapter assigned to the scope.
    pub adapter_id: AdapterId,
    /// Scope root relative to the project root.
    pub scope_root: PathBuf,
    /// Adapter-specific options.
    #[serde(default)]
    pub adapter_options: AdapterOptions,
}

/// Normalized discovery response emitted by every adapter.
#[derive(Debug, Clone, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct DiscoverResponse {
    /// Discovery schema version.
    pub schema_version: u16,
    /// Scope that owns the discovered modules.
    pub scope_id: ScopeId,
    /// Adapter that produced the response.
    pub adapter_id: AdapterId,
    /// Modules discovered in the scope.
    pub modules: Vec<DiscoveredModule>,
}

/// Module data emitted by adapter discovery before engine planning migration.
#[derive(Debug, Clone, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct DiscoveredModule {
    /// Scope that owns the module.
    pub scope_id: ScopeId,
    /// Adapter that discovered the module.
    pub adapter_id: AdapterId,
    /// Stable module name within the scope.
    pub name: ModuleId,
    /// Optional package name used by command templates.
    pub package: Option<String>,
    /// Module root relative to the project root.
    pub root: PathBuf,
    /// Module dependencies within the current scope.
    pub dependencies: Vec<ModuleId>,
    /// Glob-like source patterns relative to the project root.
    pub source_patterns: Vec<String>,
    /// Adapter-specific reporting/debug metadata.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, serde_json::Value>,
}

impl DiscoveredModule {
    /// Wrap the current engine module shape with discovery scope metadata.
    #[must_use]
    pub fn from_module(module: Module, scope_id: ScopeId, adapter_id: AdapterId) -> Self {
        Self {
            scope_id,
            adapter_id,
            name: module.name,
            package: module.package,
            root: module.root,
            dependencies: module.dependencies,
            source_patterns: module.source_patterns,
            metadata: BTreeMap::new(),
        }
    }

    /// Convert into the current engine module shape.
    #[must_use]
    pub fn into_module(self) -> Module {
        Module {
            name: self.name,
            package: self.package,
            root: self.root,
            dependencies: self.dependencies,
            source_patterns: self.source_patterns,
        }
    }
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

pub(crate) fn validate_discovery_response(
    field: impl AsRef<str>,
    request: &DiscoverRequest,
    response: &DiscoverResponse,
) -> AppResult<()> {
    let field = field.as_ref();
    if response.schema_version != DISCOVERY_SCHEMA_VERSION {
        return Err(AppError::invalid_input(
            field,
            format!(
                "unsupported discovery response schema {}",
                response.schema_version
            ),
        ));
    }
    if response.scope_id != request.scope_id {
        return Err(AppError::invalid_input(
            field,
            format!(
                "discovery response scope '{}' does not match request scope '{}'",
                response.scope_id, request.scope_id
            ),
        ));
    }
    if response.adapter_id != request.adapter_id {
        return Err(AppError::invalid_input(
            field,
            format!(
                "discovery response adapter '{}' does not match request adapter '{}'",
                response.adapter_id, request.adapter_id
            ),
        ));
    }
    for (index, module) in response.modules.iter().enumerate() {
        if module.scope_id != request.scope_id {
            return Err(AppError::invalid_input(
                field,
                format!(
                    "discovery response module {index} scope '{}' does not match request scope '{}'",
                    module.scope_id, request.scope_id
                ),
            ));
        }
        if module.adapter_id != request.adapter_id {
            return Err(AppError::invalid_input(
                field,
                format!(
                    "discovery response module {index} adapter '{}' does not match request adapter '{}'",
                    module.adapter_id, request.adapter_id
                ),
            ));
        }
    }
    Ok(())
}
