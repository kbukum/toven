//! Project-level model.

use std::path::PathBuf;

use crate::core::{ExecutionMode, ExecutionUnit, Task};

/// Project workspace to plan.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Workspace {
    /// Configuration schema version.
    pub schema: u16,
    /// Human-readable workspace name.
    pub name: String,
    /// Absolute or invocation-relative workspace root.
    pub root: PathBuf,
    /// Default git baseline reference for affected detection.
    pub base_ref: Option<String>,
    /// Profiles defined for the workspace.
    pub profiles: Vec<Profile>,
}

/// Language profile that owns tasks and discovery settings.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Profile {
    /// Profile name from the config table.
    pub name: String,
    /// Language identifier.
    pub language: String,
    /// Optional command adapter override.
    pub discovery_command: Option<Vec<String>>,
    /// Execution mode for tasks in this profile.
    pub execution: ExecutionMode,
    /// Template for rendering one module selector.
    pub module_arg_template: Vec<String>,
    /// Resource group template used for scheduling/reporting.
    pub resource_group: String,
    /// Tasks configured for this profile.
    pub tasks: Vec<Task>,
}

/// Complete plan emitted by `toven plan`.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Plan {
    /// Workspace metadata.
    pub workspace: Workspace,
    /// Execution units in release order.
    pub units: Vec<ExecutionUnit>,
}
