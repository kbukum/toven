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
    ExecutionMode, ExecutionUnit, Module, ModuleId, NodeState, Plan, Profile, Task, TaskCommand,
    Workspace,
};
pub use preset::PresetDefinition;
pub use protocol::{DISCOVERY_SCHEMA_VERSION, DiscoverRequest, DiscoverResponse};
pub use template::{Placeholder, Template, TemplatePart};

pub(crate) fn validate_name(field: impl AsRef<str>, value: &str) -> AppResult<()> {
    validation::validate_name(field, value)
}

pub(crate) fn validate_identifier(field: impl AsRef<str>, value: &str) -> AppResult<()> {
    validation::validate_identifier(field, value)
}

pub(crate) fn validate_command_template(
    field: impl AsRef<str>,
    values: &[String],
) -> AppResult<()> {
    validation::validate_command_template(field, values)
}

pub(crate) fn validate_templates(field: impl AsRef<str>, values: &[String]) -> AppResult<()> {
    validation::validate_templates(field, values)
}

pub(crate) fn validate_template(field: impl AsRef<str>, value: &str) -> AppResult<()> {
    validation::validate_template(field, value)
}

pub(crate) fn validate_discovery_request_schema(
    field: impl AsRef<str>,
    request: &DiscoverRequest,
) -> AppResult<()> {
    protocol::validate_discovery_request_schema(field, request)
}
