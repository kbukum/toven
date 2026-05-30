//! Language-agnostic module model.

use std::{fmt, path::PathBuf, str::FromStr};

use crate::core::{AppError, AppResult, PresetDefinition};

/// Unique module identifier within a workspace.
#[derive(
    Debug, Clone, Eq, Ord, PartialEq, PartialOrd, Hash, serde::Deserialize, serde::Serialize,
)]
#[serde(try_from = "String", into = "String")]
pub struct ModuleId(String);

impl ModuleId {
    /// Create a module identifier from a validated string.
    pub fn new(value: impl Into<String>) -> AppResult<Self> {
        Self::parse(value)
    }

    /// Return the identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Parse and validate a module identifier.
    pub fn parse(value: impl Into<String>) -> AppResult<Self> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(AppError::invalid_input(
                "module.name",
                "module name cannot be empty",
            ));
        }
        if value != value.trim() {
            return Err(AppError::invalid_input(
                "module.name",
                "module name cannot contain leading or trailing whitespace",
            ));
        }
        Ok(Self(value))
    }
}

impl TryFrom<String> for ModuleId {
    type Error = AppError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<ModuleId> for String {
    fn from(value: ModuleId) -> Self {
        value.0
    }
}

impl fmt::Display for ModuleId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for ModuleId {
    type Err = AppError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// A discovered module independent of language-specific manifests.
#[derive(Debug, Clone, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Module {
    /// Unique module identifier.
    pub name: ModuleId,
    /// Optional package name used by command templates.
    pub package: Option<String>,
    /// Module root relative to the workspace root.
    pub root: PathBuf,
    /// Module identifiers this module depends on.
    pub dependencies: Vec<ModuleId>,
    /// Glob-like source patterns relative to the workspace root.
    pub source_patterns: Vec<String>,
}

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

/// Configured task.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Task {
    /// Task name.
    pub name: String,
    /// Command source.
    pub command: TaskCommand,
    /// Whether passthrough arguments are allowed to participate in cache keys.
    pub cache_passthrough: bool,
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
    /// Profile name.
    pub profile: String,
    /// Task name.
    pub task: String,
    /// Source metadata for the command.
    pub command_origin: CommandOrigin,
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
    /// Whether passthrough arguments are allowed to participate in cache keys.
    pub cache_passthrough: bool,
    /// Preset-scoped input paths that affect every module using this unit.
    pub shared_inputs: Vec<String>,
}

/// Complete plan emitted by `toven plan`.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Plan {
    /// Workspace metadata.
    pub workspace: Workspace,
    /// Execution units in release order.
    pub units: Vec<ExecutionUnit>,
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::ModuleId;

    #[test]
    fn module_id_exposes_value() {
        let id = ModuleId::new("core").expect("module id parses");

        assert_eq!(id.as_str(), "core");
    }

    #[test]
    fn module_id_parse_rejects_empty_values() {
        let error = ModuleId::parse(" ").expect_err("empty value should fail");

        assert!(error.message.contains("module name"));
    }

    #[test]
    fn module_id_parse_rejects_surrounding_whitespace() {
        let error = ModuleId::parse(" api ").expect_err("surrounding whitespace should fail");

        assert!(error.message.contains("leading or trailing whitespace"));
    }

    #[test]
    fn module_id_implements_from_str() {
        let id = ModuleId::from_str("api").expect("module id parses");

        assert_eq!(id.to_string(), "api");
    }

    #[test]
    fn module_id_try_from_rejects_empty_values() {
        let error = ModuleId::try_from(String::from(" ")).expect_err("empty value should fail");

        assert!(error.message.contains("module name"));
    }

    #[test]
    fn module_id_try_from_rejects_surrounding_whitespace() {
        let error = ModuleId::try_from(String::from(" api "))
            .expect_err("surrounding whitespace should fail");

        assert!(error.message.contains("leading or trailing whitespace"));
    }

    #[test]
    fn module_id_deserialization_rejects_empty_values() {
        use serde::Deserialize as _;

        let deserializer =
            serde::de::value::StringDeserializer::<serde::de::value::Error>::new(" ".to_string());
        let error = ModuleId::deserialize(deserializer).expect_err("empty value should fail");

        assert!(error.to_string().contains("module name"));
    }

    #[test]
    fn module_id_deserialization_rejects_surrounding_whitespace() {
        use serde::Deserialize as _;

        let deserializer = serde::de::value::StringDeserializer::<serde::de::value::Error>::new(
            " api ".to_string(),
        );
        let error =
            ModuleId::deserialize(deserializer).expect_err("surrounding whitespace should fail");

        assert!(error.to_string().contains("leading or trailing whitespace"));
    }
}
