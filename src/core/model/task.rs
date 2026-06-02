//! Task-level model.

use crate::core::{AdapterId, Module, PresetDefinition, ScopeId};

/// Adapter-provided command used to contribute toolchain version identity to cache keys.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ToolchainProbe {
    /// Stable label for the probe.
    pub label: String,
    /// Program to execute.
    pub program: String,
    /// Arguments passed to the program.
    pub args: Vec<String>,
}

/// Configured task.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Task {
    /// Task name.
    pub name: String,
    /// Command source.
    pub command: TaskCommand,
    /// Task definition source.
    pub origin: TaskOrigin,
    /// Whether passthrough arguments are included in cache keys.
    pub cache_args: bool,
    /// Workspace-relative paths that affect every module using this task.
    pub shared_inputs: Vec<String>,
    /// Whether this task starts a long-lived process.
    pub persistent: bool,
    /// Readiness condition for persistent tasks.
    pub readiness: PersistentReadiness,
    /// Maximum time to wait for persistent readiness.
    pub readiness_timeout: std::time::Duration,
}

/// Source of a task definition before command rendering.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum TaskOrigin {
    /// Task was supplied by an adapter default.
    AdapterDefault {
        /// Adapter that supplied the task.
        adapter_id: AdapterId,
    },
    /// Task was defined in a project profile.
    ProjectDefault,
    /// Task was defined by a named scope override.
    ScopeOverride {
        /// Scope override that supplied the task.
        scope_id: ScopeId,
    },
}

/// Readiness condition for long-lived task processes.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub enum PersistentReadiness {
    /// The task is ready once the subprocess starts.
    #[default]
    Started,
    /// Run a bounded health command after the subprocess starts.
    Command(Vec<String>),
    /// Wait until stdout or stderr contains the literal text.
    OutputContains(String),
}

/// Source of a task command.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum TaskCommand {
    /// Resolve a named preset for the profile language.
    Preset(String),
    /// Use direct argv from config.
    Argv(Vec<String>),
    /// Fully resolved preset definition.
    ResolvedPreset(PresetDefinition),
}

/// Renderable source metadata for a planned command.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum CommandOrigin {
    /// Command argv was defined directly in project config.
    DirectArgv,
    /// Command argv came from a resolved preset.
    Preset {
        /// Preset name requested by the task.
        name: String,
        /// Preset language.
        language: String,
    },
}

/// How ready modules become execution units.
#[derive(Debug, Clone, Copy, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionMode {
    /// One command per ready module.
    SpawnEach,
    /// One command per compatible ready set.
    BatchReady,
    /// One command for the whole workspace.
    WorkspaceOnce,
}

impl std::fmt::Display for ExecutionMode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::SpawnEach => "spawn-each",
            Self::BatchReady => "batch-ready",
            Self::WorkspaceOnce => "workspace-once",
        })
    }
}

/// Scheduler node state.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum NodeState {
    /// Waiting for prerequisites.
    Pending,
    /// Available to plan.
    Ready,
    /// Satisfied by an execution unit or cache hit.
    Satisfied,
}

/// Planned command unit.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ExecutionUnit {
    /// Stable unit identifier.
    pub id: String,
    /// Scope that owns the unit.
    pub scope_id: ScopeId,
    /// Adapter that owns the unit.
    pub adapter_id: AdapterId,
    /// Task name.
    pub task: String,
    /// Source metadata for the command.
    pub command_origin: CommandOrigin,
    /// Source metadata for the task definition.
    pub task_origin: TaskOrigin,
    /// Execution mode.
    pub mode: ExecutionMode,
    /// Resource group after template rendering.
    pub resource_group: String,
    /// Modules covered by this unit.
    pub modules: Vec<Module>,
    /// Argv template to render.
    pub argv_template: Vec<String>,
    /// Per-module selector template.
    pub module_arg_template: Vec<String>,
    /// Extra user args injected through `{args}`.
    pub passthrough_args: Vec<String>,
    /// Adapter-provided toolchain probes included in cache keys.
    pub toolchain_probes: Vec<ToolchainProbe>,
    /// Whether passthrough arguments are included in cache keys.
    pub cache_args: bool,
    /// Whether this unit starts a long-lived process.
    pub persistent: bool,
    /// Readiness condition for persistent units.
    pub readiness: PersistentReadiness,
    /// Maximum time to wait for persistent readiness.
    pub readiness_timeout: std::time::Duration,
    /// Preset-scoped input paths that affect every module using this unit.
    pub shared_inputs: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::ExecutionMode;

    #[test]
    fn execution_mode_displays_kebab_case_names() {
        assert_eq!(ExecutionMode::SpawnEach.to_string(), "spawn-each");
        assert_eq!(ExecutionMode::BatchReady.to_string(), "batch-ready");
        assert_eq!(ExecutionMode::WorkspaceOnce.to_string(), "workspace-once");
    }
}
