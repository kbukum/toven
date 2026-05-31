//! Core planning model.

pub mod module;
pub mod project;
pub mod scope;
pub mod task;

pub use module::{Module, ModuleId};
pub use project::{Plan, Profile, ScopeOverride, Workspace};
pub use scope::{AdapterId, ScopeId};
pub use task::{
    CommandOrigin, ExecutionMode, ExecutionUnit, NodeState, PersistentReadiness, Task, TaskCommand,
    TaskOrigin,
};
