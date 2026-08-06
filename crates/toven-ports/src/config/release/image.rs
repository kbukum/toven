//! Container-image vocabulary: the tagged image built, pushed to a primary
//! registry plus mirrors, and signed (`[…release].image`).

use rskit_errors::{AppError, AppResult};
use rskit_util::Template;
use rskit_validation::input::validate_safe_path;
use serde::{Deserialize, Serialize};

use crate::template::ReleaseVar;

/// Default image tag template when none is configured: the released version.
const DEFAULT_TAG_TEMPLATE: &str = "{version}";

/// The `[…release].image` sub-config: how a module's container image is built,
/// tagged, pushed, and signed.
///
/// A module that runs the image phase requires this block; the phase is
/// unusable without it. `registry` is the primary registry and `mirrors` the
/// additional registries the same digest is pushed to. `name`/`tag` are
/// [`ReleaseVar`] templates (reusing the tag-template vocabulary), `context`
/// and `dockerfile` locate the build, and `sign` (keyless cosign, default
/// `true`) selects whether the pushed digest is signed.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ImageConfig {
    /// Primary registry the image is pushed to (e.g. `ghcr.io/acme`).
    pub registry: String,
    /// Additional mirror registries the same digest is pushed to; empty = none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mirrors: Vec<String>,
    /// Image name template (e.g. `toven`, `{module}`).
    pub name: String,
    /// Image tag template; `None` = the default `{version}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// Project-relative build context; `None` = the project root (`.`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Project-relative Dockerfile path; `None` = the builder default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dockerfile: Option<String>,
    /// Whether the pushed digest is signed (keyless cosign); default `true`.
    #[serde(default = "default_sign", skip_serializing_if = "is_true")]
    pub sign: bool,
}

impl Default for ImageConfig {
    fn default() -> Self {
        Self {
            registry: String::new(),
            mirrors: Vec::new(),
            name: String::new(),
            tag: None,
            context: None,
            dockerfile: None,
            sign: true,
        }
    }
}

impl ImageConfig {
    /// The resolved tag template, defaulting to `{version}` when unset.
    #[must_use]
    pub fn tag_template(&self) -> &str {
        self.tag.as_deref().unwrap_or(DEFAULT_TAG_TEMPLATE)
    }

    /// Validate every field value beyond serde's type checks.
    ///
    /// # Errors
    /// Rejects a blank primary registry, image name, or mirror; a malformed
    /// name/tag template (unknown placeholder); and an unsafe build-context or
    /// Dockerfile path.
    pub fn validate(&self, field: &str) -> AppResult<()> {
        if self.registry.trim().is_empty() {
            return Err(AppError::invalid_input(
                format!("{field}.registry"),
                "must not be blank",
            ));
        }
        if self.name.trim().is_empty() {
            return Err(AppError::invalid_input(
                format!("{field}.name"),
                "must not be blank",
            ));
        }
        for (index, mirror) in self.mirrors.iter().enumerate() {
            if mirror.trim().is_empty() {
                return Err(AppError::invalid_input(
                    format!("{field}.mirrors[{index}]"),
                    "must not be blank",
                ));
            }
        }
        validate_template(&format!("{field}.name"), &self.name)?;
        validate_template(&format!("{field}.tag"), self.tag_template())?;
        for (value, key) in [(&self.context, "context"), (&self.dockerfile, "dockerfile")] {
            if let Some(value) = value {
                if value.trim().is_empty() {
                    return Err(AppError::invalid_input(
                        format!("{field}.{key}"),
                        "must not be blank",
                    ));
                }
                validate_safe_path(value).map_err(|error| {
                    AppError::invalid_input(format!("{field}.{key}"), error.to_string())
                        .with_cause(error)
                })?;
            }
        }
        Ok(())
    }
}

/// Validate a name/tag template against the [`ReleaseVar`] vocabulary.
fn validate_template(field: &str, value: &str) -> AppResult<()> {
    Template::parse(value, ReleaseVar::ALL).map_err(|error| {
        AppError::invalid_input(field, format!("invalid image template: {error}")).with_cause(error)
    })?;
    Ok(())
}

const fn default_sign() -> bool {
    true
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_true(value: &bool) -> bool {
    *value
}

#[cfg(test)]
mod tests {
    use super::ImageConfig;

    fn parse(toml: &str) -> Result<ImageConfig, toml::de::Error> {
        toml::from_str(toml)
    }

    #[test]
    fn parses_the_image_surface() {
        let config = parse(
            r#"
            registry = "ghcr.io/acme"
            mirrors = ["docker.io/acme"]
            name = "toven"
            tag = "{version}"
            context = "services/api"
            dockerfile = "services/api/Dockerfile"
            sign = true
            "#,
        )
        .expect("parses");

        assert_eq!(config.registry, "ghcr.io/acme");
        assert_eq!(config.mirrors, ["docker.io/acme"]);
        assert_eq!(config.name, "toven");
        assert_eq!(config.tag.as_deref(), Some("{version}"));
        assert_eq!(config.context.as_deref(), Some("services/api"));
        assert!(config.sign);
        config
            .validate("ecosystems.rust.release.image")
            .expect("valid");
    }

    #[test]
    fn sign_defaults_to_true_and_tag_defaults_to_version() {
        let config = parse(
            r#"
            registry = "ghcr.io/acme"
            name = "toven"
            "#,
        )
        .expect("parses");
        assert!(config.sign, "signing is the keyless default");
        assert_eq!(config.tag_template(), "{version}");
        config
            .validate("ecosystems.rust.release.image")
            .expect("valid");
    }

    #[test]
    fn rejects_unknown_field() {
        assert!(
            parse(
                r#"registry = "ghcr.io/acme"
        name = "toven"
        bogus = true"#
            )
            .is_err()
        );
    }

    #[test]
    fn validate_rejects_blank_registry() {
        let config = parse(
            r#"registry = "  "
        name = "toven""#,
        )
        .expect("parses");
        let error = config
            .validate("ecosystems.rust.release.image")
            .expect_err("blank registry rejected");
        assert!(error.to_string().contains("registry"), "{error}");
    }

    #[test]
    fn validate_rejects_blank_name() {
        let config = parse(
            r#"registry = "ghcr.io/acme"
        name = "  ""#,
        )
        .expect("parses");
        let error = config
            .validate("ecosystems.rust.release.image")
            .expect_err("blank name rejected");
        assert!(error.to_string().contains("name"), "{error}");
    }

    #[test]
    fn validate_rejects_blank_mirror() {
        let config = parse(
            r#"registry = "ghcr.io/acme"
        name = "toven"
        mirrors = ["docker.io/acme", "  "]"#,
        )
        .expect("parses");
        let error = config
            .validate("ecosystems.rust.release.image")
            .expect_err("blank mirror rejected");
        assert!(error.to_string().contains("mirrors[1]"), "{error}");
    }

    #[test]
    fn validate_rejects_malformed_name_template() {
        let config = parse(
            r#"registry = "ghcr.io/acme"
        name = "{modual}""#,
        )
        .expect("parses");
        let error = config
            .validate("ecosystems.rust.release.image")
            .expect_err("bad placeholder rejected");
        assert!(error.to_string().contains("name"), "{error}");
    }

    #[test]
    fn validate_rejects_traversing_context_path() {
        let config = parse(
            r#"registry = "ghcr.io/acme"
        name = "toven"
        context = "../../etc""#,
        )
        .expect("parses");
        let error = config
            .validate("ecosystems.rust.release.image")
            .expect_err("traversing context rejected");
        assert!(error.to_string().contains("context"), "{error}");
    }
}
