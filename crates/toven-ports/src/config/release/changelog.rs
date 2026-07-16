//! Changelog vocabulary: the changelog path and whether it is required.

use rskit_errors::AppError;
use rskit_errors::AppResult;
use rskit_validation::input::validate_safe_path;
use serde::{Deserialize, Serialize};

/// Changelog generation settings the release changelog step reads.
///
/// `path` is the workspace-relative changelog file (default `CHANGELOG.md`);
/// `required` fails a release readiness/plan when a changed module has no
/// changelog entry, so a release cannot ship an undocumented change.
#[derive(Debug, Clone, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChangelogConfig {
    /// Workspace-relative changelog path; `None` uses the adapter default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Whether a changelog entry is required for a changed module.
    #[serde(default, skip_serializing_if = "is_false")]
    pub required: bool,
}

impl ChangelogConfig {
    /// Validate the changelog path as a safe workspace-relative path.
    ///
    /// # Errors
    /// Rejects an absolute or traversing changelog path.
    pub fn validate(&self, field: &str) -> AppResult<()> {
        if let Some(path) = &self.path {
            validate_safe_path(path).map_err(|error| {
                AppError::invalid_input(format!("{field}.path"), error.to_string())
                    .with_cause(error)
            })?;
        }
        Ok(())
    }
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_false(value: &bool) -> bool {
    !*value
}
