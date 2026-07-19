//! The coverage gate: compare a module's metrics against its resolved thresholds
//! and produce a typed verdict.
//!
//! A dimension is checked only when both a threshold is configured **and** the
//! metric was measured — a Go module never fails a `function` floor it cannot
//! measure. A below-threshold dimension fails the gate closed under `Block`,
//! is reported without failing under `Advisory`, and is measured-only for a
//! module in the ecosystem `exclude` list.

use toven_model::ModuleKey;
use toven_ports::{CoverageThresholds, Enforcement};

use super::metrics::CoverageMetrics;
use super::settings::ResolvedCoverageSettings;

/// A measured coverage dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverageDimension {
    /// Absolute line coverage.
    Line,
    /// Function coverage (Rust-only).
    Function,
    /// Region coverage (Rust-only).
    Region,
    /// Changed-scope line coverage (`--changed`).
    ChangedLine,
}

impl CoverageDimension {
    /// The canonical report name for the dimension.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Line => "line",
            Self::Function => "function",
            Self::Region => "region",
            Self::ChangedLine => "changed-line",
        }
    }
}

/// One dimension's measured value against its threshold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DimensionOutcome {
    /// The gated dimension.
    pub dimension: CoverageDimension,
    /// The measured percentage.
    pub measured: f64,
    /// The configured floor.
    pub threshold: f64,
    /// Whether the measured value meets the floor.
    pub passed: bool,
}

/// A module's overall gate status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleStatus {
    /// Every gated dimension met its floor.
    Passed,
    /// A dimension fell below its floor and enforcement is `block`.
    Failed,
    /// A dimension fell below its floor but enforcement is `advisory`.
    Advisory,
    /// The module is measured but excluded from gating.
    Excluded,
}

impl ModuleStatus {
    /// The canonical report name for the status.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Advisory => "advisory",
            Self::Excluded => "excluded",
        }
    }

    /// Whether this status fails the overall gate.
    #[must_use]
    pub const fn is_failure(self) -> bool {
        matches!(self, Self::Failed)
    }
}

/// One module's coverage verdict: its metrics, per-dimension outcomes, and status.
#[derive(Debug, Clone, PartialEq)]
pub struct ModuleCoverage {
    /// The module the verdict is for.
    pub module: ModuleKey,
    /// The measured metrics.
    pub metrics: CoverageMetrics,
    /// The enforcement applied.
    pub enforcement: Enforcement,
    /// Per-dimension outcomes, in report order.
    pub outcomes: Vec<DimensionOutcome>,
    /// The overall module status.
    pub status: ModuleStatus,
}

/// Gate a module's `metrics` against its resolved `settings`.
#[must_use]
pub(super) fn gate_module(
    module: ModuleKey,
    metrics: CoverageMetrics,
    settings: &ResolvedCoverageSettings,
) -> ModuleCoverage {
    let outcomes = evaluate(metrics, &settings.thresholds);
    let any_failed = outcomes.iter().any(|outcome| !outcome.passed);
    let status = if settings.excluded {
        ModuleStatus::Excluded
    } else if !any_failed {
        ModuleStatus::Passed
    } else if settings.enforcement.is_block() {
        ModuleStatus::Failed
    } else {
        ModuleStatus::Advisory
    };

    ModuleCoverage {
        module,
        metrics,
        enforcement: settings.enforcement,
        outcomes,
        status,
    }
}

/// Build the per-dimension outcomes for every configured-and-measured dimension.
fn evaluate(metrics: CoverageMetrics, thresholds: &CoverageThresholds) -> Vec<DimensionOutcome> {
    let checks = [
        (CoverageDimension::Line, thresholds.line, Some(metrics.line)),
        (
            CoverageDimension::Function,
            thresholds.function,
            metrics.function,
        ),
        (CoverageDimension::Region, thresholds.region, metrics.region),
        (
            CoverageDimension::ChangedLine,
            thresholds.changed_line,
            metrics.changed_line,
        ),
    ];

    let mut outcomes = Vec::new();
    for (dimension, threshold, measured) in checks {
        if let (Some(threshold), Some(measured)) = (threshold, measured) {
            outcomes.push(DimensionOutcome {
                dimension,
                measured,
                threshold,
                passed: measured + f64::EPSILON >= threshold,
            });
        }
    }
    outcomes
}

#[cfg(test)]
mod tests {
    use super::{ModuleStatus, gate_module};
    use crate::coverage::metrics::CoverageMetrics;
    use crate::coverage::settings::ResolvedCoverageSettings;
    use toven_model::{EcosystemId, ModuleKey, ModuleRef};
    use toven_ports::{CoverageThresholds, Enforcement};

    fn key(name: &str) -> ModuleKey {
        ModuleKey::bare(ModuleRef::new(EcosystemId::new("rust").unwrap(), name).unwrap())
    }

    fn metrics(line: f64) -> CoverageMetrics {
        CoverageMetrics {
            line,
            function: None,
            region: None,
            changed_line: None,
        }
    }

    fn settings(
        line: Option<f64>,
        enforcement: Enforcement,
        excluded: bool,
    ) -> ResolvedCoverageSettings {
        ResolvedCoverageSettings {
            thresholds: CoverageThresholds {
                line,
                ..CoverageThresholds::default()
            },
            enforcement,
            excluded,
        }
    }

    #[test]
    fn passes_when_above_floor() {
        let verdict = gate_module(
            key("a"),
            metrics(95.0),
            &settings(Some(90.0), Enforcement::Block, false),
        );
        assert_eq!(verdict.status, ModuleStatus::Passed);
    }

    #[test]
    fn block_below_floor_is_a_failure() {
        let verdict = gate_module(
            key("a"),
            metrics(80.0),
            &settings(Some(90.0), Enforcement::Block, false),
        );
        assert_eq!(verdict.status, ModuleStatus::Failed);
        assert!(verdict.status.is_failure());
    }

    #[test]
    fn advisory_below_floor_reports_without_failing() {
        let verdict = gate_module(
            key("a"),
            metrics(80.0),
            &settings(Some(90.0), Enforcement::Advisory, false),
        );
        assert_eq!(verdict.status, ModuleStatus::Advisory);
        assert!(!verdict.status.is_failure());
    }

    #[test]
    fn excluded_module_is_measured_not_gated() {
        let verdict = gate_module(
            key("a"),
            metrics(10.0),
            &settings(Some(90.0), Enforcement::Block, true),
        );
        assert_eq!(verdict.status, ModuleStatus::Excluded);
    }

    #[test]
    fn unmeasured_dimension_is_skipped() {
        // a function floor with no measured function coverage never fails.
        let mut settings = settings(None, Enforcement::Block, false);
        settings.thresholds.function = Some(90.0);
        let verdict = gate_module(key("a"), metrics(100.0), &settings);
        assert_eq!(verdict.status, ModuleStatus::Passed);
        assert!(verdict.outcomes.is_empty());
    }
}
