//! Release hooks vocabulary: recognized task references run before/after a
//! release.

use rskit_errors::{AppError, AppResult};
use serde::{Deserialize, Serialize};

/// Optional pre/post release hooks, each a recognized task reference.
///
/// Hooks are **task names** the engine already knows (argv-first, no shell
/// unless a task opts in), so a user composes custom release steps from the
/// same task model that drives every other verb. Both lists are empty by
/// default.
#[derive(Debug, Clone, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HooksConfig {
    /// Task references run before the release mutation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pre: Vec<String>,
    /// Task references run after a successful release.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub post: Vec<String>,
}

impl HooksConfig {
    /// Validate that every hook names a non-blank task reference.
    ///
    /// # Errors
    /// Rejects a blank hook task reference.
    pub fn validate(&self, field: &str) -> AppResult<()> {
        for (phase, hooks) in [("pre", &self.pre), ("post", &self.post)] {
            for (index, hook) in hooks.iter().enumerate() {
                if hook.trim().is_empty() {
                    return Err(AppError::invalid_input(
                        format!("{field}.{phase}[{index}]"),
                        "hook task reference must not be blank",
                    ));
                }
            }
        }
        Ok(())
    }
}
