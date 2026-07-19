//! The `[…coverage]` sub-config: the declarative coverage-gating surface, shared
//! by the ecosystem default (`[ecosystems.<id>].coverage`) and the per-module
//! override (`[modules.<name>.coverage]`).

use std::collections::BTreeMap;

use rskit_errors::{AppError, AppResult};
use serde::{Deserialize, Serialize};

use super::{CoverageProfile, CoverageThresholds, Enforcement};

/// The declarative coverage surface: thresholds, enforcement, and scope inputs
/// only — never the runner flags.
///
/// Toven config owns the pass/fail verdict inputs (the per-dimension floors, the
/// enforcement mode, and which modules to exclude or elevate); the *measurement*
/// flags (`--html`, profraw cleanup, the tool's own `--jobs`, the profile output
/// path) stay in the coverage **task's argv** the user authors. Every field is
/// optional/defaulted, so an existing `toven.toml` with no `[…coverage]` block
/// keeps parsing and inherits the adapter default. The engine folds ecosystem →
/// profile → per-module override into a resolved settings value with the
/// precedence `[modules.<name>.coverage]` > `profiles.<name>` >
/// `[ecosystems.<id>].coverage` > adapter default.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageConfig {
    /// Absolute per-module line-coverage floor (codecov's `project`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<f64>,
    /// Absolute per-module function-coverage floor; `None` where unmeasured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<f64>,
    /// Absolute per-module region-coverage floor; `None` where unmeasured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<f64>,
    /// Changed-scope line-coverage floor applied under `--changed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_line: Option<f64>,
    /// Enforcement mode; `None` = adapter default (`block`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enforcement: Option<Enforcement>,
    /// Module names measured but never gated.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    /// Named elevated threshold sets resolved above the ecosystem default.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, CoverageProfile>,
}

impl CoverageConfig {
    /// Whether this config is entirely default (so it can be skipped on serialize).
    #[must_use]
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }

    /// The top-level threshold floors as a resolvable value.
    #[must_use]
    pub const fn thresholds(&self) -> CoverageThresholds {
        CoverageThresholds {
            line: self.line,
            function: self.function,
            region: self.region,
            changed_line: self.changed_line,
        }
    }

    /// The first profile that names `module`, in declaration order.
    #[must_use]
    pub fn profile_for(&self, module: &str) -> Option<&CoverageProfile> {
        self.profiles
            .values()
            .find(|profile| profile.applies_to(module))
    }

    /// Whether `module` (an ecosystem-local name) is excluded from gating.
    #[must_use]
    pub fn is_excluded(&self, module: &str) -> bool {
        self.exclude.iter().any(|name| name == module)
    }

    /// Validate every field value beyond serde's type checks.
    ///
    /// `field` is the config path prefix used in diagnostics (e.g.
    /// `ecosystems.rust.coverage` or `modules.rust:core.coverage`).
    ///
    /// # Errors
    /// Rejects an out-of-range threshold, a blank exclude/profile-module name,
    /// and a profile that names no module.
    pub fn validate(&self, field: &str) -> AppResult<()> {
        self.thresholds().validate(field)?;
        for (index, module) in self.exclude.iter().enumerate() {
            if module.trim().is_empty() {
                return Err(AppError::invalid_input(
                    format!("{field}.exclude[{index}]"),
                    "module name must not be blank",
                ));
            }
        }
        for (name, profile) in &self.profiles {
            profile.validate(&format!("{field}.profiles.{name}"))?;
            if profile.modules.is_empty() {
                return Err(AppError::invalid_input(
                    format!("{field}.profiles.{name}.modules"),
                    "a coverage profile must name at least one module",
                ));
            }
        }
        Ok(())
    }

    /// Validate a per-module `[modules.<ref>.coverage]` override.
    ///
    /// A per-module override may set only the threshold floors and the
    /// enforcement mode. `exclude` and `profiles` are ecosystem-level decisions
    /// resolved from `[ecosystems.<id>].coverage` — they never affect gating
    /// inside a single module's block, so accepting them silently would be a
    /// footgun. They are rejected here rather than ignored.
    ///
    /// # Errors
    /// Rejects a per-module `exclude` or `profiles`, then defers to
    /// [`validate`](Self::validate) for the shared field checks.
    pub fn validate_module_override(&self, field: &str) -> AppResult<()> {
        if !self.exclude.is_empty() {
            return Err(AppError::invalid_input(
                format!("{field}.exclude"),
                "exclude is an ecosystem-level setting; it has no effect in a per-module coverage override",
            ));
        }
        if !self.profiles.is_empty() {
            return Err(AppError::invalid_input(
                format!("{field}.profiles"),
                "profiles is an ecosystem-level setting; it has no effect in a per-module coverage override",
            ));
        }
        self.validate(field)
    }
}

#[cfg(test)]
mod tests {
    use super::{CoverageConfig, Enforcement};

    fn parse(toml: &str) -> Result<CoverageConfig, toml::de::Error> {
        toml::from_str(toml)
    }

    #[test]
    fn empty_block_is_all_default() {
        let config = parse("").expect("parses");
        assert!(config.is_default());
        config.validate("ecosystems.rust.coverage").expect("valid");
    }

    #[test]
    fn parses_the_full_surface() {
        let config = parse(
            r#"
            line = 90.0
            function = 85.0
            region = 80.0
            changed_line = 85.0
            enforcement = "block"
            exclude = ["toven-suite"]

            [profiles.security]
            line = 95.0
            modules = ["toven-auth", "toven-authz"]
            "#,
        )
        .expect("parses");

        assert_eq!(config.line, Some(90.0));
        assert_eq!(config.enforcement, Some(Enforcement::Block));
        assert_eq!(config.exclude, ["toven-suite"]);
        let security = config.profile_for("toven-auth").expect("profile matches");
        assert_eq!(security.line, Some(95.0));
        assert!(config.is_excluded("toven-suite"));
        config.validate("ecosystems.rust.coverage").expect("valid");
    }

    #[test]
    fn rejects_unknown_field() {
        let error = parse("bogus = true").expect_err("unknown field rejected");
        assert!(error.to_string().contains("bogus"), "{error}");
    }

    #[test]
    fn rejects_unknown_enforcement_variant() {
        assert!(parse(r#"enforcement = "warn""#).is_err());
    }

    #[test]
    fn validate_rejects_out_of_range_threshold() {
        let config = parse("line = 140.0").expect("parses");
        let error = config
            .validate("ecosystems.rust.coverage")
            .expect_err("out-of-range rejected");
        assert!(error.to_string().contains("line"), "{error}");
    }

    #[test]
    fn validate_rejects_profile_without_modules() {
        let config = parse(
            r"
            [profiles.security]
            line = 95.0
            ",
        )
        .expect("parses");
        assert!(config.validate("ecosystems.rust.coverage").is_err());
    }

    #[test]
    fn module_override_rejects_exclude() {
        let config = parse(r#"exclude = ["toven-suite"]"#).expect("parses");
        let error = config
            .validate_module_override("modules.rust:core.coverage")
            .expect_err("per-module exclude rejected");
        assert!(error.to_string().contains("exclude"), "{error}");
    }

    #[test]
    fn module_override_rejects_profiles() {
        let config = parse(
            r#"
            [profiles.security]
            line = 95.0
            modules = ["toven-auth"]
            "#,
        )
        .expect("parses");
        let error = config
            .validate_module_override("modules.rust:core.coverage")
            .expect_err("per-module profiles rejected");
        assert!(error.to_string().contains("profiles"), "{error}");
    }

    #[test]
    fn module_override_accepts_thresholds_and_enforcement() {
        let config = parse(
            r#"
            line = 85.0
            enforcement = "advisory"
            "#,
        )
        .expect("parses");
        config
            .validate_module_override("modules.rust:core.coverage")
            .expect("threshold-only override is valid");
    }
}
