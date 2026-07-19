//! Coverage profile vocabulary: one elevated threshold set applied to a named
//! group of modules (rskit's `[security] { packages, line }`).

use rskit_errors::{AppError, AppResult};
use serde::{Deserialize, Serialize};

use super::{CoverageThresholds, Enforcement};

/// One `[…coverage.profiles.<name>]` entry: an elevated threshold set applied
/// to the listed modules.
///
/// A profile resolves **below** a per-module override and **above** the
/// ecosystem default, so a module named in a profile inherits the profile's
/// floors unless its own `[modules.<name>.coverage]` overrides them. `modules`
/// lists the module names (the ecosystem-local name, e.g. `toven-auth`) the
/// profile applies to; the threshold/enforcement fields are inline so the
/// section stays `deny_unknown_fields`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageProfile {
    /// Absolute per-module line-coverage floor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<f64>,
    /// Absolute per-module function-coverage floor (Rust-only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<f64>,
    /// Absolute per-module region-coverage floor (Rust-only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<f64>,
    /// Changed-scope line-coverage floor applied under `--changed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_line: Option<f64>,
    /// Enforcement override for the profile's modules; `None` inherits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enforcement: Option<Enforcement>,
    /// The module names the profile's thresholds apply to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<String>,
}

impl CoverageProfile {
    /// The inline threshold floors as a resolvable value.
    #[must_use]
    pub const fn thresholds(&self) -> CoverageThresholds {
        CoverageThresholds {
            line: self.line,
            function: self.function,
            region: self.region,
            changed_line: self.changed_line,
        }
    }

    /// Whether `module` (an ecosystem-local name) is covered by this profile.
    #[must_use]
    pub fn applies_to(&self, module: &str) -> bool {
        self.modules.iter().any(|name| name == module)
    }

    /// Validate the thresholds and the module list.
    ///
    /// # Errors
    /// Rejects an out-of-range threshold or a blank module name.
    pub fn validate(&self, field: &str) -> AppResult<()> {
        self.thresholds().validate(field)?;
        for (index, module) in self.modules.iter().enumerate() {
            if module.trim().is_empty() {
                return Err(AppError::invalid_input(
                    format!("{field}.modules[{index}]"),
                    "module name must not be blank",
                ));
            }
        }
        Ok(())
    }
}
