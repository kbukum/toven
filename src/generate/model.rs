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
    /// Adapter discovery options.
    pub discovery: BTreeMap<String, TomlValue>,
}

/// Minimal TOML value tree needed by generated fragments.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum TomlValue {
    /// String scalar.
    String(String),
    /// Array value.
    Array(Vec<TomlValue>),
}
