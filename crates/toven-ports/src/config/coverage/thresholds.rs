//! Threshold vocabulary: the per-dimension coverage floors shared by the
//! ecosystem default, an elevated profile, and a per-module override.

use rskit_errors::{AppError, AppResult};

/// The per-dimension coverage floors, as a resolvable value (not a serde
/// section — the `[…coverage]` block and its profiles inline these fields so
/// they stay `deny_unknown_fields`).
///
/// Each floor is an optional percentage in `0.0..=100.0`. `line` is the absolute
/// per-module line floor (codecov's `project`); `function`/`region` are the
/// Rust-only dimensions llvm-cov emits, left `None` where an ecosystem cannot
/// measure them; `changed_line` is the floor applied to the changed scope under
/// `--changed` (codecov's `patch`). A `None` dimension is not gated.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CoverageThresholds {
    /// Absolute per-module line-coverage floor.
    pub line: Option<f64>,
    /// Absolute per-module function-coverage floor (Rust-only).
    pub function: Option<f64>,
    /// Absolute per-module region-coverage floor (Rust-only).
    pub region: Option<f64>,
    /// Changed-scope line-coverage floor applied under `--changed`.
    pub changed_line: Option<f64>,
}

impl CoverageThresholds {
    /// Whether no dimension is set (so a scope inherits its parent thresholds).
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.line.is_none()
            && self.function.is_none()
            && self.region.is_none()
            && self.changed_line.is_none()
    }

    /// Fold `over` onto `self`: a set dimension in `over` replaces this one, an
    /// unset dimension inherits (the documented per-module > profile > ecosystem
    /// precedence, one dimension at a time).
    #[must_use]
    pub fn merge(&self, over: &Self) -> Self {
        Self {
            line: over.line.or(self.line),
            function: over.function.or(self.function),
            region: over.region.or(self.region),
            changed_line: over.changed_line.or(self.changed_line),
        }
    }

    /// Validate every set dimension as a percentage in `0.0..=100.0`.
    ///
    /// # Errors
    /// Rejects a threshold outside `0.0..=100.0` or a non-finite value.
    pub fn validate(&self, field: &str) -> AppResult<()> {
        for (name, value) in [
            ("line", self.line),
            ("function", self.function),
            ("region", self.region),
            ("changed_line", self.changed_line),
        ] {
            if let Some(value) = value {
                validate_percentage(&format!("{field}.{name}"), value)?;
            }
        }
        Ok(())
    }
}

/// Reject a coverage percentage outside the `0.0..=100.0` range.
fn validate_percentage(field: &str, value: f64) -> AppResult<()> {
    if !value.is_finite() || !(0.0..=100.0).contains(&value) {
        return Err(AppError::invalid_input(
            field,
            format!("coverage threshold must be a percentage in 0.0..=100.0, got {value}"),
        ));
    }
    Ok(())
}
