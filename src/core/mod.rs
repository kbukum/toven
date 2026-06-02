//! Core contracts and product model.

mod adapter;
mod error;
pub mod model;
mod preset;
pub(crate) mod process_config;
pub mod protocol;
mod template;
mod validation;

pub use adapter::DiscoveryAdapter;
pub use error::{AppError, AppResult, ErrorCode};
pub use model::{
    AdapterId, CacheLocation, CacheSettings, CommandOrigin, DependencyOverlay, ExecutionMode,
    ExecutionUnit, Module, ModuleId, NodeState, PersistentReadiness, Plan, Profile, ScopeId,
    ScopeOverride, ScopedModuleKey, Task, TaskCommand, TaskOrigin, Workspace,
    scoped_module_display, scoped_module_key,
};
pub use preset::PresetDefinition;
pub use protocol::{
    AdapterOptions, DISCOVERY_SCHEMA_VERSION, DiscoverRequest, DiscoverResponse, DiscoveredModule,
};
pub(crate) use protocol::{validate_discovery_request_schema, validate_discovery_response};
pub use template::{Placeholder, Template, TemplatePart};
pub(crate) use validation::{
    validate_command_template, validate_identifier, validate_name, validate_shared_inputs,
    validate_template, validate_templates,
};
