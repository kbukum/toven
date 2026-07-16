//! Prerelease vocabulary: the recognized channel set and the optional
//! branch→channel mapping.

use std::collections::BTreeMap;

use rskit_errors::{AppError, AppResult};
use serde::{Deserialize, Serialize};

/// Prerelease channels and the branch→channel mapping.
///
/// `channels` names the recognized prerelease trains (`rc`, `alpha`, `beta`) that
/// `--pre <channel>` and release-branch workflows resolve against.
/// `branch_channels` maps a release branch to the channel it cuts, so pushing to
/// a `next` branch can imply a `beta` train without a per-run flag. Both are
/// empty by default (stable-only releases).
#[derive(Debug, Clone, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PrereleaseConfig {
    /// Recognized prerelease channel identifiers, each a valid semver prerelease
    /// segment set (e.g. `rc`, `alpha`, `beta`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<String>,
    /// Optional map from release branch name to the channel it releases on.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub branch_channels: BTreeMap<String, String>,
}

impl PrereleaseConfig {
    /// Whether `channel` is one of the configured channels.
    #[must_use]
    pub fn recognizes(&self, channel: &str) -> bool {
        self.channels.iter().any(|known| known == channel)
    }

    /// Validate the channel names and the branch→channel mapping.
    ///
    /// # Errors
    /// Rejects an empty or non-semver channel identifier, an empty branch name,
    /// and a branch mapped to a channel that is not in `channels`.
    pub fn validate(&self, field: &str) -> AppResult<()> {
        for channel in &self.channels {
            validate_channel(&format!("{field}.channels"), channel)?;
        }
        for (branch, channel) in &self.branch_channels {
            if branch.trim().is_empty() {
                return Err(AppError::invalid_input(
                    format!("{field}.branch_channels"),
                    "release branch name must not be empty",
                ));
            }
            if !self.recognizes(channel) {
                return Err(AppError::invalid_input(
                    format!("{field}.branch_channels.{branch}"),
                    format!("branch maps to undeclared channel '{channel}'"),
                ));
            }
        }
        Ok(())
    }
}

/// Validate a single prerelease channel identifier as a semver prerelease
/// segment set: non-empty, dot-separated, each segment `[0-9A-Za-z-]+`.
fn validate_channel(field: &str, channel: &str) -> AppResult<()> {
    if channel.is_empty() {
        return Err(AppError::invalid_input(
            field,
            "prerelease channel must not be empty",
        ));
    }
    for segment in channel.split('.') {
        if segment.is_empty()
            || !segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(AppError::invalid_input(
                field,
                format!(
                    "invalid prerelease channel '{channel}': segments must be non-empty and use only [0-9A-Za-z-]"
                ),
            ));
        }
    }
    Ok(())
}
