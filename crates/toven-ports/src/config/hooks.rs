//! Lifecycle hooks vocabulary: recognized task references run before/after a
//! verb's mutation.
//!
//! This is a **verb-agnostic** concern. A project attaches [`HooksConfig`] to
//! any verb through the project-level `[hooks.<verb>]` map, letting a user
//! compose pre/post steps from the same task model that drives every other verb.

use rskit_errors::{AppError, AppResult};
use serde::{Deserialize, Serialize};

/// Optional pre/post lifecycle hooks, each a recognized task reference.
///
/// Hooks are **task names** the engine already knows (argv-first, no shell
/// unless a task opts in), so a user composes custom steps from the same task
/// model that drives every other verb. A `pre` hook runs before the verb's
/// mutation and fails the verb closed if it fails (nothing is mutated); a `post`
/// hook runs after a successful mutation. Both lists are empty by default.
#[derive(Debug, Clone, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HooksConfig {
    /// Task references run before the verb's mutation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pre: Vec<String>,
    /// Task references run after the verb's mutation succeeds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub post: Vec<String>,
}

impl HooksConfig {
    /// Whether no hook is configured (so a caller can skip all hook wiring).
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.pre.is_empty() && self.post.is_empty()
    }

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
