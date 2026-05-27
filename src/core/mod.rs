//! Core contracts and product model.

mod adapter;
mod error;
mod model;
mod preset;
mod protocol;
mod template;

pub use adapter::LangAdapter;
pub use error::{AppError, AppResult, ErrorCode};
pub use model::{
    ExecutionMode, ExecutionUnit, Module, ModuleId, NodeState, Plan, Profile, Task, TaskCommand,
    Workspace,
};
pub use preset::PresetDefinition;
pub use protocol::{DiscoverRequest, DiscoverResponse};
pub use template::{Placeholder, Template, TemplatePart};

include!("validation.rs");
