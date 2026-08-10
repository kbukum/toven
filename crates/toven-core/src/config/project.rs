//! `[project]` — repo identity and the change baseline.

use serde::{Deserialize, Serialize};

/// The reserved `[project]` section: repo identity and branching baseline.
///
/// In a single-repo workspace this is the canonical identity; in a multi-repo
/// umbrella it is the degenerate single-member case alongside `[[members]]`
/// during federation composition. `base_ref` lives here because the change
/// baseline is a property of repo branching, not an engine tuning knob.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    /// Required human-facing project name.
    pub name: String,
    /// Workspace root relative to the config file (defaults to `.`).
    #[serde(default = "default_root")]
    pub root: String,
    /// Git ref the change baseline is computed against (e.g. `origin/main`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_ref: Option<String>,
}

/// The default workspace root: the directory holding `toven.toml`.
fn default_root() -> String {
    ".".to_string()
}

impl ProjectConfig {
    /// Borrow the configured workspace root.
    #[must_use]
    pub fn root(&self) -> &str {
        &self.root
    }
}
