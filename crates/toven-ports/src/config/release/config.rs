//! The `[…release]` sub-config: the full declarative release surface, shared by
//! the ecosystem default (`[ecosystems.<id>].release`) and the per-module
//! override (`[modules.<name>.release]`).

use rskit_errors::{AppError, AppResult};
use rskit_util::Template;
use serde::{Deserialize, Serialize};
use toven_model::Entrypoint;

use crate::config::HooksConfig;
use crate::release::Visibility;
use crate::template::ReleaseVar;

use super::{
    BumpLevel, ChangelogConfig, DependentVersion, HostConfig, ImageConfig, PhasesConfig,
    PrereleaseConfig, PublicationPolicy, SignConfig,
};

/// The declarative release surface (`[ecosystems.<id>].release` and the
/// per-module `[modules.<name>.release]` override).
///
/// Every field is optional with a documented default, so an existing
/// `toven.toml` keeps parsing unchanged and an unset override field inherits
/// the ecosystem default (and, in turn, the built-in adapter default). The
/// engine folds ecosystem → per-module override into a resolved settings value
/// with the precedence `[modules.<name>.release]` > `[ecosystems.<id>].release` >
/// adapter default (the per-run bump argv layers on top).
#[derive(Debug, Clone, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseConfig {
    /// Named bump policy (e.g. `"semver-cascade"`); `None` = adapter default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy: Option<String>,
    /// Default bump level applied to a changed module; `None` = adapter
    /// default.
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
    /// Whether release tags are signed. Signing is always annotated, so
    /// `sign_tags = true` requires `tag_message` to be set (a signed lightweight
    /// tag is not a thing) and an available signing key (`user.signingkey`).
    /// `None` = adapter default (`false`, unsigned).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sign_tags: Option<bool>,
    /// Signing backend for signed tags, mapped onto git's `gpg.format`: one of
    /// `openpgp` (or the `gpg` alias), `ssh`, or `x509`. `None` inherits the
    /// repository's `gpg.format`. Only applies when `sign_tags = true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sign_format: Option<String>,
    /// Signing key for signed tags, mapped onto git's `user.signingkey`. Carries
    /// the key *identifier* only — never key material. `None` inherits the
    /// repository's `user.signingkey`. Only applies when `sign_tags = true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signing_key: Option<String>,
    /// Release commit message template; `None` = adapter default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_message: Option<String>,
    /// Changelog generation settings; `None` = inherit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changelog: Option<ChangelogConfig>,
    /// Whether the release commit/tags are pushed; `None` = adapter default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push: Option<bool>,
    /// Whether the release commit's branch is pushed alongside the tags. When
    /// `false`, only the release tags are pushed and the branch ref is left
    /// untouched — the tag-only mode required by a protected release branch
    /// whose commit lands through a pull request. `None` = adapter default
    /// (`true`, push the branch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push_branch: Option<bool>,
    /// Git remote pushed to; `None` = adapter default (`origin`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// Allowed release branches; `Some([])` clears to any branch, `None` =
    /// inherit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branches: Option<Vec<String>>,
    /// Target registry identifier (e.g. `"crates-io"`); `None` = not
    /// publishable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
    /// Whether the module publishes to its registry; `publish = false` makes a
    /// registry-less release tag-only. `None` = default (publishes when a
    /// `registry` is set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish: Option<bool>,
    /// Whether the module is excluded from the release entirely (no version
    /// change, tag, target call, or hosted release). `None` = not excluded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude: Option<bool>,
    /// Skip registry lookups and anchor idempotency on release tags only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offline: Option<bool>,
    /// Environment-variable name holding the registry token (never the secret).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_env: Option<String>,
    /// Exposure the release is cut with (`public`/`private`/`internal`); `None`
    /// = default (`public`). Enforced fail-closed at the registry-publish
    /// boundary (a non-public release to a public-only registry is rejected at
    /// plan time and by the registry adapter). The tag push and hosted forge
    /// Release follow the remote repository's exposure, so they carry this as
    /// recorded intent, not a per-Release forge flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<Visibility>,
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
    /// Hosted forge Release settings (the phase after tag/registry publish);
    /// `None` = inherit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<HostConfig>,
    /// Container-image phase settings (build, push to a primary registry plus
    /// mirrors, sign the digest); `None` = the module runs no image phase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<ImageConfig>,
    /// Per-phase backing map: how each release phase is satisfied (native, the
    /// default, or delegated to an external tool). `None` = inherit; an absent
    /// phase entry runs natively.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phases: Option<PhasesConfig>,
    /// Who cuts the release: `toven` (the default — Toven owns the whole flow
    /// and creates the tag/hosted Release itself) or `maintainer` (a human
    /// created the tag/Release in the forge and Toven runs against them,
    /// verifying rather than creating the tag, then publishing + attaching +
    /// attesting). `None` = adapter default (`toven`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<Entrypoint>,
    /// Whether this module is the release train's **umbrella** — an aggregate
    /// that represents the whole release rather than a separately-published
    /// unit. An umbrella module contributes its members' notes to the shared
    /// hosted Release and does not publish to a registry unless it is itself a
    /// registry package (`registry` + `publish`). `None` = not an umbrella.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub umbrella: Option<bool>,
}

impl ReleaseConfig {
    /// Whether this config is entirely default (so it can be skipped on
    /// serialize).
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
    /// Rejects a malformed tag/commit template (unknown placeholder), an
    /// invalid prerelease channel or branch mapping, an unsafe changelog path,
    /// a blank strategy/registry/remote/token-env/branch/readiness/hook
    /// reference, and an inconsistent signing selection.
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
        if let Some(host) = &self.host {
            host.validate(&format!("{field}.host"))?;
        }
        if let Some(image) = &self.image {
            image.validate(&format!("{field}.image"))?;
        }
        if let Some(phases) = &self.phases {
            phases.validate(&format!("{field}.phases"))?;
        }
        validate_optional_nonblank(&format!("{field}.strategy"), self.strategy.as_deref())?;
        validate_optional_nonblank(&format!("{field}.registry"), self.registry.as_deref())?;
        PublicationPolicy::validate_fields(
            field,
            self.registry.as_deref(),
            self.publish,
            self.exclude,
        )?;
        if self.exclude == Some(true)
            && let Some(host) = &self.host
            && host
                .assets
                .as_ref()
                .is_some_and(|assets| !assets.is_empty())
        {
            return Err(AppError::invalid_input(
                format!("{field}.exclude"),
                "an excluded module cannot declare hosted release assets; remove the host assets \
                 or set exclude = false",
            ));
        }
        if self.exclude == Some(true) && self.image.is_some() {
            return Err(AppError::invalid_input(
                format!("{field}.image"),
                "an excluded module runs no image phase, so it cannot declare an image block; \
                 remove the image block or set exclude = false",
            ));
        }
        if self.exclude == Some(true) && self.umbrella == Some(true) {
            return Err(AppError::invalid_input(
                format!("{field}.umbrella"),
                "an excluded module takes no part in the release, so it cannot be the release \
                 umbrella; remove umbrella or set exclude = false",
            ));
        }
        validate_optional_nonblank(&format!("{field}.remote"), self.remote.as_deref())?;
        validate_optional_nonblank(&format!("{field}.token_env"), self.token_env.as_deref())?;
        validate_optional_nonblank(&format!("{field}.sign_format"), self.sign_format.as_deref())?;
        validate_optional_nonblank(&format!("{field}.signing_key"), self.signing_key.as_deref())?;
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
    if value.trim().is_empty() {
        return Err(AppError::invalid_input(field, "must not be blank"));
    }
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
        assert_eq!(config.sign_tags, Some(true));
        assert_eq!(config.sign_format.as_deref(), Some("openpgp"));
        assert_eq!(config.signing_key.as_deref(), Some("ABCD1234"));
        assert_eq!(
            config.branches.as_deref(),
            Some(["main".into(), "release".into()].as_slice())
        );
        assert_eq!(
            config.prerelease.as_ref().expect("prerelease set").channels,
            ["rc", "beta"]
        );
        let changelog = config.changelog.as_ref().expect("changelog set");
        assert!(changelog.required);
        assert!(changelog.roll);
        assert!(config.sign.as_ref().expect("sign set").enabled);
        assert_eq!(config.hooks.as_ref().expect("hooks set").pre, ["fmt-check"]);
        assert_eq!(
            config.host.as_ref().expect("host set").forge.as_deref(),
            Some("github")
        );
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

    #[test]
    fn validate_rejects_blank_template() {
        let config = parse(r#"tag_format = "   ""#).expect("parses");
        let error = config
            .validate("ecosystems.rust.release")
            .expect_err("blank template rejected");
        assert!(error.to_string().contains("tag_format"), "{error}");
    }

    #[test]
    fn validate_accepts_a_valid_image_block() {
        let config = parse(
            r#"
            [image]
            registry = "ghcr.io/acme"
            name = "toven"
            "#,
        )
        .expect("parses");
        config.validate("ecosystems.rust.release").expect("valid");
        assert_eq!(
            config.image.as_ref().expect("image set").registry,
            "ghcr.io/acme"
        );
    }

    #[test]
    fn validate_propagates_an_invalid_image_block() {
        let config = parse(
            r#"
            [image]
            registry = "ghcr.io/acme"
            name = "{modual}"
            "#,
        )
        .expect("parses");
        let error = config
            .validate("ecosystems.rust.release")
            .expect_err("bad image template rejected");
        assert!(error.to_string().contains("image.name"), "{error}");
    }

    #[test]
    fn validate_rejects_an_image_block_on_an_excluded_module() {
        let config = parse(
            r#"
            exclude = true

            [image]
            registry = "ghcr.io/acme"
            name = "toven"
            "#,
        )
        .expect("parses");
        let error = config
            .validate("modules.rust:demo.release")
            .expect_err("image on an excluded module rejected");
        assert!(error.to_string().contains("image"), "{error}");
        assert!(error.to_string().contains("excluded"), "{error}");
    }

    #[test]
    fn parses_entrypoint_and_umbrella() {
        use toven_model::Entrypoint;

        let config = parse(
            r#"
            entrypoint = "maintainer"
            umbrella = true
            "#,
        )
        .expect("parses");
        assert_eq!(config.entrypoint, Some(Entrypoint::Maintainer));
        assert_eq!(config.umbrella, Some(true));
        config.validate("ecosystems.rust.release").expect("valid");
    }

    #[test]
    fn entrypoint_defaults_to_none_and_rejects_unknown() {
        let config = parse("").expect("parses");
        assert_eq!(config.entrypoint, None);
        assert_eq!(config.umbrella, None);
        let error = parse(r#"entrypoint = "ci""#).expect_err("unknown entrypoint rejected");
        assert!(error.to_string().contains("entrypoint"), "{error}");
    }

    #[test]
    fn validate_rejects_an_umbrella_on_an_excluded_module() {
        let config = parse(
            r"
            exclude = true
            umbrella = true
            ",
        )
        .expect("parses");
        let error = config
            .validate("modules.rust:suite.release")
            .expect_err("umbrella on an excluded module rejected");
        assert!(error.to_string().contains("umbrella"), "{error}");
        assert!(error.to_string().contains("excluded"), "{error}");
    }
}
