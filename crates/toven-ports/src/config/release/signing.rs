//! Signing vocabulary: whether release artifacts are signed and by which signer.

use rskit_errors::{AppError, AppResult};
use serde::{Deserialize, Serialize};

/// Artifact-signing settings consumed by the signed-artifact release flow.
///
/// `enabled` toggles signing; `signer` names the signer identity/key selection
/// (never the secret itself). Signing is off by default.
#[derive(Debug, Clone, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SignConfig {
    /// Whether release artifacts are signed.
    #[serde(default, skip_serializing_if = "is_false")]
    pub enabled: bool,
    /// Signer identity/key selection (never a secret); `None` = signer default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signer: Option<String>,
}

impl SignConfig {
    /// Whether every field is at its default (so it can be skipped on serialize).
    #[must_use]
    pub const fn is_default(&self) -> bool {
        !self.enabled && self.signer.is_none()
    }

    /// Validate the signer selection.
    ///
    /// # Errors
    /// Rejects a blank signer or a signer named while signing is disabled.
    pub fn validate(&self, field: &str) -> AppResult<()> {
        if let Some(signer) = &self.signer {
            if signer.trim().is_empty() {
                return Err(AppError::invalid_input(
                    format!("{field}.signer"),
                    "signer must not be blank",
                ));
            }
            if !self.enabled {
                return Err(AppError::invalid_input(
                    format!("{field}.signer"),
                    "signer is set but signing is disabled (set enabled = true)",
                ));
            }
        }
        Ok(())
    }
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_false(value: &bool) -> bool {
    !*value
}
