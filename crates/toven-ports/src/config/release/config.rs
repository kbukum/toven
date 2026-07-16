//! The `[…release]` sub-config: the full declarative release surface, shared by
//! the ecosystem default (`[ecosystems.<id>].release`) and the per-module
//! override (`[modules.<name>.release]`).

use rskit_errors::{AppError, AppResult};
use rskit_util::Template;
use serde::{Deserialize, Serialize};

use crate::template::ReleaseVar;

use super::{
    BumpLevel, ChangelogConfig, DependentVersion, HooksConfig, PrereleaseConfig, SignConfig,
};

/// The declarative release surface (`[ecosystems.<id>].release` and the
/// per-module `[modules.<name>.release]` override).
///
/// Every field is optional with a documented default, so an existing
/// `toven.toml` keeps parsing unchanged and an unset override field inherits the
/// ecosystem default (and, in turn, the built-in adapter default). The engine
/// folds ecosystem → per-module override into a resolved settings value with the
/// precedence `[modules.<name>.release]` > `[ecosystems.<id>].release` >
/// adapter default (the per-run bump argv layers on top).
#[derive(Debug, Clone, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseConfig {
    /// Named bump policy (e.g. `"semver-cascade"`); `None` = adapter default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy: Option<String>,
    /// Default bump level applied to a changed module; `None` = adapter default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<BumpLevel>,
    /// How a dependency-floor bump cascades into dependents; `None` = default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependent_version: Option<DependentVersion>,
    /// Prerelease channels and the branch→channel mapping; `None` = inherit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prerelease: Option<PrereleaseConfig>,
    /// Release tag name template (e.g. `v{version}`, `{module}/v{version}`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag_format: Option<String>,
    /// Annotated-tag message template; `None` = a lightweight tag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag_message: Option<String>,
    /// Release commit message template; `None` = adapter default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_message: Option<String>,
    /// Changelog generation settings; `None` = inherit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changelog: Option<ChangelogConfig>,
    /// Whether the release commit/tags are pushed; `None` = adapter default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push: Option<bool>,
    /// Git remote pushed to; `None` = adapter default (`origin`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// Allowed release branches; `Some([])` clears to any branch, `None` = inherit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branches: Option<Vec<String>>,
    /// Target registry identifier (e.g. `"crates-io"`); `None` = not publishable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
    /// Skip registry lookups and anchor idempotency on release tags only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offline: Option<bool>,
    /// Environment-variable name holding the registry token (never the secret).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_env: Option<String>,
    /// Artifact-signing settings; `None` = inherit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sign: Option<SignConfig>,
    /// Recognized checks composing `release readiness`; `Some([])` clears,
    /// `None` = inherit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readiness: Option<Vec<String>>,
    /// Optional pre/post release hooks (recognized task references); `None` =
    /// inherit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hooks: Option<HooksConfig>,
}

impl ReleaseConfig {
    /// Whether this config is entirely default (so it can be skipped on serialize).
    #[must_use]
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }

    /// Validate every field value beyond serde's type/variant checks.
    ///
    /// `field` is the config path prefix used in diagnostics (e.g.
    /// `ecosystems.rust.release` or `modules.rust:core.release`).
    ///
    /// # Errors
    /// Rejects a malformed tag/commit template (unknown placeholder), an invalid
    /// prerelease channel or branch mapping, an unsafe changelog path, a blank
    /// strategy/registry/remote/token-env/branch/readiness/hook reference, and an
    /// inconsistent signing selection.
    pub fn validate(&self, field: &str) -> AppResult<()> {
        for (name, template) in [
            ("tag_format", &self.tag_format),
            ("tag_message", &self.tag_message),
            ("commit_message", &self.commit_message),
        ] {
            if let Some(value) = template {
                validate_template(&format!("{field}.{name}"), value)?;
            }
        }
        if let Some(prerelease) = &self.prerelease {
            prerelease.validate(&format!("{field}.prerelease"))?;
        }
        if let Some(changelog) = &self.changelog {
            changelog.validate(&format!("{field}.changelog"))?;
        }
        if let Some(sign) = &self.sign {
            sign.validate(&format!("{field}.sign"))?;
        }
        if let Some(hooks) = &self.hooks {
            hooks.validate(&format!("{field}.hooks"))?;
        }
        validate_optional_nonblank(&format!("{field}.strategy"), self.strategy.as_deref())?;
        validate_optional_nonblank(&format!("{field}.registry"), self.registry.as_deref())?;
        validate_optional_nonblank(&format!("{field}.remote"), self.remote.as_deref())?;
        validate_optional_nonblank(&format!("{field}.token_env"), self.token_env.as_deref())?;
        if let Some(branches) = &self.branches {
            validate_nonblank_entries(&format!("{field}.branches"), branches)?;
        }
        if let Some(readiness) = &self.readiness {
            validate_nonblank_entries(&format!("{field}.readiness"), readiness)?;
        }
        Ok(())
    }
}

/// Validate a tag/commit template against the [`ReleaseVar`] vocabulary.
fn validate_template(field: &str, value: &str) -> AppResult<()> {
    Template::parse(value, ReleaseVar::ALL).map_err(|error| {
        AppError::invalid_input(field, format!("invalid release template: {error}"))
            .with_cause(error)
    })?;
    Ok(())
}

/// Reject a present-but-blank optional string field.
fn validate_optional_nonblank(field: &str, value: Option<&str>) -> AppResult<()> {
    if let Some(value) = value
        && value.trim().is_empty()
    {
        return Err(AppError::invalid_input(field, "must not be blank"));
    }
    Ok(())
}

/// Reject any blank entry in a string list.
fn validate_nonblank_entries(field: &str, entries: &[String]) -> AppResult<()> {
    for (index, entry) in entries.iter().enumerate() {
        if entry.trim().is_empty() {
            return Err(AppError::invalid_input(
                format!("{field}[{index}]"),
                "must not be blank",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ReleaseConfig;
    use crate::config::{BumpLevel, DependentVersion};

    fn parse(toml: &str) -> Result<ReleaseConfig, toml::de::Error> {
        toml::from_str(toml)
    }

    #[test]
    fn parses_the_full_release_surface() {
        let config = parse(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/config/release/full-surface.toml"
        )))
        .expect("parses");

        assert_eq!(config.level, Some(BumpLevel::Minor));
        assert_eq!(config.dependent_version, Some(DependentVersion::Upgrade));
        assert_eq!(config.tag_format.as_deref(), Some("{module}/v{version}"));
        assert_eq!(config.branches.as_deref(), Some(["main".into(), "release".into()].as_slice()));
        assert_eq!(config.prerelease.as_ref().expect("prerelease set").channels, ["rc", "beta"]);
        assert!(config.changelog.as_ref().expect("changelog set").required);
        assert!(config.sign.as_ref().expect("sign set").enabled);
        assert_eq!(config.hooks.as_ref().expect("hooks set").pre, ["fmt-check"]);
        config.validate("ecosystems.rust.release").expect("valid");
    }

    #[test]
    fn empty_release_block_is_all_default() {
        let config = parse("").expect("parses");
        assert!(config.is_default());
        config.validate("ecosystems.rust.release").expect("valid");
    }

    #[test]
    fn rejects_unknown_field() {
        let error = parse("bogus = true").expect_err("unknown field rejected");
        assert!(error.to_string().contains("bogus"), "{error}");
    }

    #[test]
    fn rejects_unknown_enum_variant() {
        assert!(parse(r#"level = "huge""#).is_err());
    }

    #[test]
    fn validate_rejects_malformed_tag_template() {
        let config = parse(r#"tag_format = "v{verison}""#).expect("parses");
        let error = config
            .validate("ecosystems.rust.release")
            .expect_err("bad placeholder rejected");
        assert!(error.to_string().contains("tag_format"), "{error}");
    }

    #[test]
    fn validate_rejects_invalid_prerelease_channel() {
        let config = parse(
            r#"
            [prerelease]
            channels = ["r c"]
            "#,
        )
        .expect("parses");
        assert!(config.validate("ecosystems.rust.release").is_err());
    }

    #[test]
    fn validate_rejects_branch_mapped_to_undeclared_channel() {
        let config = parse(
            r#"
            [prerelease]
            channels = ["rc"]
            branch_channels = { next = "beta" }
            "#,
        )
        .expect("parses");
        assert!(config.validate("ecosystems.rust.release").is_err());
    }

    #[test]
    fn validate_rejects_signer_without_signing() {
        let config = parse(
            r#"
            [sign]
            signer = "bot"
            "#,
        )
        .expect("parses");
        assert!(config.validate("ecosystems.rust.release").is_err());
    }

    #[test]
    fn validate_rejects_blank_registry() {
        let config = parse(r#"registry = "   ""#).expect("parses");
        let error = config
            .validate("ecosystems.rust.release")
            .expect_err("blank registry rejected");
        assert!(error.to_string().contains("registry"), "{error}");
    }

    #[test]
    fn validate_rejects_blank_strategy() {
        let config = parse(r#"strategy = " ""#).expect("parses");
        assert!(config.validate("ecosystems.rust.release").is_err());
    }
}
