//! Hosted-release vocabulary: the forge Release cut after tag and registry
//! publish (`[…release].host`).

use rskit_errors::{AppError, AppResult};
use rskit_validation::input::validate_safe_path;
use serde::{Deserialize, Serialize};

/// The `[…release].host` sub-config: which forge to cut a hosted Release on and
/// how to shape it.
///
/// Every field is optional, but a Release-shaping field only takes effect once
/// `forge` selects a hosted Release. With no `forge`, the release pipeline stops
/// after tag and registry publish — a hosted Release is opt-in, and setting any
/// shaping field without a forge is rejected. An unset `draft` / `prerelease`
/// inherits the engine default (`prerelease` derives from the released version's
/// prerelease channel), unset `notes` sources the release notes from the
/// changelog, and unset `assets` uploads nothing.
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
    /// Rejects a blank forge/notes value, any asset path that is blank,
    /// absolute, or escapes the workspace via traversal, and any Release-shaping
    /// field (`draft`/`prerelease`/`notes`/`assets`) set while `forge` is unset.
    pub fn validate(&self, field: &str) -> AppResult<()> {
        if let Some(forge) = &self.forge {
            if forge.trim().is_empty() {
                return Err(AppError::invalid_input(
                    format!("{field}.forge"),
                    "must not be blank",
                ));
            }
        } else if let Some(shaping) = self.shaping_field() {
            return Err(AppError::invalid_input(
                format!("{field}.{shaping}"),
                format!("{shaping} is set but no forge is configured (set {field}.forge)"),
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

    /// Name of the first Release-shaping field that is set, if any.
    ///
    /// These fields only take effect once a `forge` selects a hosted Release, so
    /// setting one without a forge is a configuration mistake rather than a
    /// silent no-op.
    const fn shaping_field(&self) -> Option<&'static str> {
        if self.draft.is_some() {
            Some("draft")
        } else if self.prerelease.is_some() {
            Some("prerelease")
        } else if self.notes.is_some() {
            Some("notes")
        } else if self.assets.is_some() {
            Some("assets")
        } else {
            None
        }
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
        let config = parse(r#"forge = "github"
        assets = ["ok", "  "]"#)
        .expect("parses");
        let error = config
            .validate("ecosystems.rust.release.host")
            .expect_err("blank asset rejected");
        assert!(error.to_string().contains("assets[1]"), "{error}");
    }

    #[test]
    fn validate_rejects_absolute_asset_path() {
        let config = parse(r#"forge = "github"
        assets = ["/etc/passwd"]"#)
        .expect("parses");
        let error = config
            .validate("ecosystems.rust.release.host")
            .expect_err("absolute asset rejected");
        assert!(error.to_string().contains("assets[0]"), "{error}");
    }

    #[test]
    fn validate_rejects_traversing_asset_path() {
        let config = parse(r#"forge = "github"
        assets = ["ok", "../../etc/passwd"]"#)
        .expect("parses");
        let error = config
            .validate("ecosystems.rust.release.host")
            .expect_err("traversing asset rejected");
        assert!(error.to_string().contains("assets[1]"), "{error}");
    }

    #[test]
    fn validate_rejects_shaping_field_without_forge() {
        for (toml, field) in [
            ("draft = true", "draft"),
            ("prerelease = true", "prerelease"),
            (r#"notes = "handcrafted""#, "notes"),
            (r#"assets = ["target/toven/release/core.cdx.json"]"#, "assets"),
        ] {
            let config = parse(toml).expect("parses");
            let error = config
                .validate("ecosystems.rust.release.host")
                .expect_err("shaping field without forge rejected");
            assert!(error.to_string().contains(field), "{error}");
            assert!(error.to_string().contains("forge"), "{error}");
        }
    }
}
