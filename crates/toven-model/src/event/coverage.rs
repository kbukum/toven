//! Coverage vocabulary carried by [`ModuleCoverageFinished`].
//!
//! Value types for the per-module coverage verdict, emitted as each module's
//! aggregation completes. Percentages are carried as integer *basis points*
//! (hundredths of a percent, `0..=10000`) rather than `f64` so the event stays
//! `Eq`-comparable and byte-stable across the serialized driver boundary; the
//! reporter renders them back to a `NN.NN%` string.
//!
//! [`ModuleCoverageFinished`]: crate::Event::ModuleCoverageFinished

use serde::{Deserialize, Serialize};

/// A gated coverage dimension.
///
/// The closed set of metrics a module can be measured on; mirrors the engine's
/// coverage dimensions but lives at L0 so the event vocabulary owns its own
/// stable, serializable names.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CoverageMetric {
    /// Absolute line coverage.
    Line,
    /// Function coverage (Rust-only).
    Function,
    /// Region coverage (Rust-only).
    Region,
    /// Changed-scope line coverage (`--changed`).
    ChangedLine,
}

/// One dimension's measured coverage against its configured floor.
///
/// `measured` and `threshold` are basis points (hundredths of a percent), so
/// `95.37%` is `9537`. `threshold` is omitted when the dimension is measured but
/// not gated (no floor configured, or measured-only for an excluded module).
#[derive(Debug, Clone, Copy, Eq, PartialEq, Deserialize, Serialize)]
pub struct CoverageMeasurement {
    /// The measured dimension.
    pub metric: CoverageMetric,
    /// The measured coverage in basis points (hundredths of a percent).
    pub measured: u32,
    /// The configured floor in basis points, when the dimension is gated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<u32>,
    /// Whether the measured value met its floor. `true` for an ungated
    /// dimension (nothing to fall below).
    pub met: bool,
}

/// A module's overall coverage verdict.
///
/// Only [`Failed`](Self::Failed) drives a non-zero exit; the other verdicts
/// feed the terminal [`OutcomeSummary`](crate::OutcomeSummary) as succeeded or
/// skipped work.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CoverageVerdict {
    /// Every gated dimension met its floor.
    Passed,
    /// A dimension fell below its floor under `block` enforcement.
    Failed,
    /// A dimension fell below its floor but enforcement is `advisory`.
    Advisory,
    /// The module is measured but excluded from gating.
    Excluded,
}

impl CoverageVerdict {
    /// Whether this verdict fails the overall gate (drives a non-zero exit).
    #[must_use]
    pub const fn is_failure(self) -> bool {
        matches!(self, Self::Failed)
    }
}

#[cfg(test)]
mod tests {
    use super::{CoverageMeasurement, CoverageMetric, CoverageVerdict};

    #[test]
    fn measurement_round_trips_and_omits_an_absent_threshold() {
        let gated = CoverageMeasurement {
            metric: CoverageMetric::Line,
            measured: 9537,
            threshold: Some(9000),
            met: true,
        };
        let json = serde_json::to_string(&gated).expect("serializes");
        assert!(json.contains("\"threshold\":9000"), "{json}");
        assert_eq!(
            serde_json::from_str::<CoverageMeasurement>(&json).expect("round-trips"),
            gated
        );

        let ungated = CoverageMeasurement {
            metric: CoverageMetric::ChangedLine,
            measured: 8800,
            threshold: None,
            met: true,
        };
        let json = serde_json::to_string(&ungated).expect("serializes");
        assert!(
            !json.contains("threshold"),
            "absent floor is omitted: {json}"
        );
        assert_eq!(
            serde_json::from_str::<CoverageMeasurement>(&json).expect("round-trips"),
            ungated
        );
    }

    #[test]
    fn metric_names_are_stable_kebab_case() {
        assert_eq!(
            serde_json::to_string(&CoverageMetric::ChangedLine).expect("serializes"),
            "\"changed-line\""
        );
    }

    #[test]
    fn only_failed_is_a_gate_failure() {
        assert!(CoverageVerdict::Failed.is_failure());
        for verdict in [
            CoverageVerdict::Passed,
            CoverageVerdict::Advisory,
            CoverageVerdict::Excluded,
        ] {
            assert!(!verdict.is_failure(), "{verdict:?} must not fail the gate");
        }
    }
}
