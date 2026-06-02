//! Typed fragments used by config generation.

use std::{collections::BTreeMap, path::PathBuf};

use crate::core::{AdapterId, AppResult, ExecutionMode};

/// Input for one generation run.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct GenerateRequest {
    /// Project root to inspect.
    pub root: PathBuf,
    /// Generated profile name.
    pub profile_name: String,
    /// Optional adapter filter.
    pub adapter: Option<AdapterId>,
    /// Adapter-specific manifest hints relative to the root.
    pub manifests: Vec<PathBuf>,
    /// Whether to write `toven.toml`.
    pub write: bool,
    /// Whether an existing `toven.toml` may be replaced.
    pub overwrite: bool,
}

/// Shared context passed to generation contributors.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct GenerateContext {
    /// Canonical project root.
    pub root: PathBuf,
    /// Generated profile name.
    pub profile_name: String,
    /// Explicit manifest hints.
    pub manifests: Vec<PathBuf>,
}

/// Adapter-owned generation contribution.
pub trait GenerateContributor {
    /// Adapter identifier this contributor generates for.
    fn adapter_id(&self) -> &AdapterId;

    /// Generate a profile fragment, or `None` when this adapter does not match the project.
    fn generate(&self, context: &mut GenerateContext) -> AppResult<Option<GeneratedProfile>>;
}

/// Generated `toven.toml` document.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct GenerateDocument {
    /// Project table.
    pub project: GeneratedProject,
    /// Profile tables keyed by profile name.
    pub profiles: BTreeMap<String, GeneratedProfile>,
    /// Warnings/suggestions that should be shown to the user.
    pub warnings: Vec<String>,
}

/// Generated `[project]` table.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct GeneratedProject {
    /// Config schema version.
    pub schema: u16,
    /// Project name.
    pub name: String,
    /// Project root as it should appear in config.
    pub root: PathBuf,
    /// Default affected baseline.
    pub base_ref: Option<String>,
}

/// Generated `[profiles.<name>]` table.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct GeneratedProfile {
    /// Profile name.
    pub name: String,
    /// Adapter id.
    pub adapter: AdapterId,
    /// Execution mode.
    pub execution: ExecutionMode,
    /// Per-module selector template.
    pub module_arg_template: Vec<String>,
    /// Resource group template.
    pub resource_group: String,
    /// Generated task definitions keyed by task name.
    pub tasks: BTreeMap<String, GeneratedTask>,
    /// Adapter discovery options.
    pub discovery: BTreeMap<String, TomlValue>,
}

/// Generated task definition.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct GeneratedTask {
    /// Direct argv template.
    pub argv: Vec<String>,
    /// Include passthrough args in cache keys instead of disabling cache.
    pub cache_args: bool,
    /// Plain workspace-relative paths that affect every module using this task.
    pub shared_inputs: Vec<String>,
    /// Whether this task starts a long-lived process.
    pub persistent: bool,
    /// Persistent task readiness shortcut.
    pub ready_on: Option<String>,
    /// Persistent task health command.
    pub ready_command: Option<Vec<String>>,
    /// Literal stdout/stderr text that marks a persistent task ready.
    pub ready_output: Option<String>,
    /// Persistent readiness timeout in seconds.
    pub ready_timeout_seconds: Option<u64>,
}

/// Minimal TOML value tree needed by generated fragments.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum TomlValue {
    /// String scalar.
    String(String),
    /// Array value.
    Array(Vec<Self>),
}
