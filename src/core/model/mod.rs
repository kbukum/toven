//! Core planning model.

pub mod cache;
pub mod module;
pub mod project;
pub mod scope;
pub mod task;

pub use cache::{CacheLocation, CacheSettings};
pub use module::{
    DependencyOverlay, Module, ModuleId, ScopedModuleKey, scoped_module_display, scoped_module_key,
};
pub use project::{Plan, Profile, ScopeOverride, Workspace};
pub use scope::{AdapterId, ScopeId};
pub use task::{
    CommandOrigin, ExecutionMode, ExecutionUnit, NodeState, PersistentReadiness, Task, TaskCommand,
    TaskOrigin,
};
