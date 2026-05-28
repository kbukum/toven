//! Core contracts and product model.

mod adapter;
mod error;
mod model;
mod preset;
mod protocol;
mod template;
mod validation;

pub use adapter::LangAdapter;
pub use error::{AppError, AppResult, ErrorCode};
pub use model::{
    CommandOrigin, ExecutionMode, ExecutionUnit, Module, ModuleId, NodeState, Plan, Profile, Task,
    TaskCommand, Workspace,
};
pub use preset::PresetDefinition;
pub(crate) use protocol::validate_discovery_request_schema;
pub use protocol::{DISCOVERY_SCHEMA_VERSION, DiscoverRequest, DiscoverResponse};
pub use template::{Placeholder, Template, TemplatePart};
pub(crate) use validation::{
    validate_command_template, validate_identifier, validate_name, validate_template,
    validate_templates,
};
