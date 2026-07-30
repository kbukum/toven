//! Signing vocabulary: whether release artifacts are signed and by which
//! signer.

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
    /// Keyless verification identity: the `certificate-identity-regexp` a
    /// downloaded signature must match (e.g. the release workflow ref). Consumed
    /// by `release verify --download`; never a secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    /// Keyless verification issuer: the `certificate-oidc-issuer` a downloaded
    /// signature's certificate must chain to (e.g.
    /// `https://token.actions.githubusercontent.com`). Consumed by
    /// `release verify --download`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
}

impl SignConfig {
    /// Validate the signer selection.
    ///
    /// # Errors
    /// Rejects a blank signer or a signer named while signing is disabled, and
    /// a blank keyless verification identity or issuer.
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
        for (value, key) in [(&self.identity, "identity"), (&self.issuer, "issuer")] {
            if let Some(value) = value
                && value.trim().is_empty()
            {
                return Err(AppError::invalid_input(
                    format!("{field}.{key}"),
                    format!("{key} must not be blank"),
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
