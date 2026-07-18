//! Hosted-release vocabulary: the forge Release cut after tag and registry
//! publish (`[…release].host`).

use rskit_errors::{AppError, AppResult};
use rskit_validation::input::validate_safe_path;
use serde::{Deserialize, Serialize};

/// The `[…release].host` sub-config: which forge to cut a hosted Release on and
/// how to shape it.
///
/// Every field is optional. With no `forge`, the release pipeline stops after
/// tag and registry publish — a hosted Release is opt-in. An unset `draft` /
/// `prerelease` inherits the engine default (`prerelease` derives from the
/// released version's prerelease channel), unset `notes` sources the release
/// notes from the changelog, and unset `assets` uploads nothing.
#[derive(Debug, Clone, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostConfig {
    /// Forge that hosts the Release (e.g. `"github"`); `None` = no hosted Release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forge: Option<String>,
    /// Whether the Release is cut as a draft; `None` = engine default (`false`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<bool>,
    /// Whether the Release is marked as a prerelease; `None` = derive from the
    /// released version's prerelease channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prerelease: Option<bool>,
    /// Explicit release-note body; `None` = source from the changelog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// Project-relative artifact paths uploaded to the Release; `None` = none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assets: Option<Vec<String>>,
}

impl HostConfig {
    /// Validate every field value beyond serde's type checks.
    ///
    /// # Errors
    /// Rejects a blank forge/notes value and any asset path that is blank,
    /// absolute, or escapes the workspace via traversal.
    pub fn validate(&self, field: &str) -> AppResult<()> {
        if let Some(forge) = &self.forge
            && forge.trim().is_empty()
        {
            return Err(AppError::invalid_input(
                format!("{field}.forge"),
                "must not be blank",
            ));
        }
        if let Some(notes) = &self.notes
            && notes.trim().is_empty()
        {
            return Err(AppError::invalid_input(
                format!("{field}.notes"),
                "must not be blank",
            ));
        }
        if let Some(assets) = &self.assets {
            for (index, asset) in assets.iter().enumerate() {
                if asset.trim().is_empty() {
                    return Err(AppError::invalid_input(
                        format!("{field}.assets[{index}]"),
                        "asset path must not be blank",
                    ));
                }
                validate_safe_path(asset).map_err(|error| {
                    AppError::invalid_input(format!("{field}.assets[{index}]"), error.to_string())
                        .with_cause(error)
                })?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::HostConfig;

    fn parse(toml: &str) -> Result<HostConfig, toml::de::Error> {
        toml::from_str(toml)
    }

    #[test]
    fn parses_the_host_surface() {
        let config = parse(
            r#"
            forge = "github"
            draft = true
            prerelease = false
            notes = "handcrafted"
            assets = ["target/toven/release/core.cdx.json"]
            "#,
        )
        .expect("parses");

        assert_eq!(config.forge.as_deref(), Some("github"));
        assert_eq!(config.draft, Some(true));
        assert_eq!(config.prerelease, Some(false));
        assert_eq!(config.notes.as_deref(), Some("handcrafted"));
        assert_eq!(
            config.assets.as_deref(),
            Some(["target/toven/release/core.cdx.json".to_string()].as_slice())
        );
        config
            .validate("ecosystems.rust.release.host")
            .expect("valid");
    }

    #[test]
    fn empty_host_block_is_all_default() {
        let config = parse("").expect("parses");
        assert_eq!(config, HostConfig::default());
        config
            .validate("ecosystems.rust.release.host")
            .expect("valid");
    }

    #[test]
    fn rejects_unknown_field() {
        assert!(parse("bogus = true").is_err());
    }

    #[test]
    fn validate_rejects_blank_forge() {
        let config = parse(r#"forge = "  ""#).expect("parses");
        let error = config
            .validate("ecosystems.rust.release.host")
            .expect_err("blank forge rejected");
        assert!(error.to_string().contains("forge"), "{error}");
    }

    #[test]
    fn validate_rejects_blank_asset_path() {
        let config = parse(r#"assets = ["ok", "  "]"#).expect("parses");
        let error = config
            .validate("ecosystems.rust.release.host")
            .expect_err("blank asset rejected");
        assert!(error.to_string().contains("assets[1]"), "{error}");
    }

    #[test]
    fn validate_rejects_absolute_asset_path() {
        let config = parse(r#"assets = ["/etc/passwd"]"#).expect("parses");
        let error = config
            .validate("ecosystems.rust.release.host")
            .expect_err("absolute asset rejected");
        assert!(error.to_string().contains("assets[0]"), "{error}");
    }

    #[test]
    fn validate_rejects_traversing_asset_path() {
        let config = parse(r#"assets = ["ok", "../../etc/passwd"]"#).expect("parses");
        let error = config
            .validate("ecosystems.rust.release.host")
            .expect_err("traversing asset rejected");
        assert!(error.to_string().contains("assets[1]"), "{error}");
    }
}
