//! Per-module coverage event projection.
//!
//! Maps each gated [`ModuleCoverage`] verdict onto an
//! [`Event::ModuleCoverageFinished`], one per module. The library returns its
//! typed [`CoverageReport`] for the summary and the exit; these emissions never
//! print.
//!
//! [`Event::ModuleCoverageFinished`]: toven_model::Event::ModuleCoverageFinished

use rskit_errors::AppResult;
use toven_model::{CoverageMeasurement, CoverageMetric, CoverageVerdict, Event};
use toven_ports::Reporter;

use super::gate::{CoverageDimension, ModuleCoverage, ModuleStatus};
use super::report::CoverageReport;

/// Emit each module's coverage verdict in report order.
///
/// Called after aggregation has gated every module, so each event carries a
/// settled measured-vs-threshold verdict, including a `Failed` one. The exit is
/// derived once from the summary, not from any individual event.
///
/// # Errors
/// Propagates a reporter sink failure.
pub(super) fn emit_verdicts(reporter: &mut dyn Reporter, report: &CoverageReport) -> AppResult<()> {
    for module in &report.modules {
        reporter.emit(&coverage_event(module))?;
    }
    Ok(())
}

/// Project one module's gate verdict onto its `ModuleCoverageFinished` event.
#[must_use]
fn coverage_event(module: &ModuleCoverage) -> Event {
    Event::ModuleCoverageFinished {
        module: module.module.to_string(),
        measurements: measurements(module),
        verdict: verdict(module.status),
    }
}

/// Build one measurement per measured dimension, annotating the configured
/// floor and pass verdict from the matching gate outcome. A dimension that was
/// measured but not gated carries no threshold and is considered met.
fn measurements(module: &ModuleCoverage) -> Vec<CoverageMeasurement> {
    let dimensions = [
        (
            CoverageMetric::Line,
            CoverageDimension::Line,
            Some(module.metrics.line),
        ),
        (
            CoverageMetric::Function,
            CoverageDimension::Function,
            module.metrics.function,
        ),
        (
            CoverageMetric::Region,
            CoverageDimension::Region,
            module.metrics.region,
        ),
        (
            CoverageMetric::ChangedLine,
            CoverageDimension::ChangedLine,
            module.metrics.changed_line,
        ),
    ];

    let mut out = Vec::new();
    for (metric, dimension, measured) in dimensions {
        let Some(measured) = measured else {
            continue;
        };
        let outcome = module
            .outcomes
            .iter()
            .find(|outcome| outcome.dimension == dimension);
        out.push(CoverageMeasurement {
            metric,
            measured: basis_points(measured),
            threshold: outcome.map(|outcome| basis_points(outcome.threshold)),
            met: outcome.is_none_or(|outcome| outcome.passed),
        });
    }
    out
}

/// Convert a `0..=100` percentage to `0..=10000` basis points (hundredths of a
/// percent), the `Eq`-stable representation the event vocabulary carries.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn basis_points(percentage: f64) -> u32 {
    // Percentages are bounded to `0..=100`, so the clamped, rounded value always
    // fits `0..=10000` — the cast neither truncates a meaningful value nor loses
    // a sign.
    (percentage * 100.0).round().clamp(0.0, f64::from(u32::MAX)) as u32
}

/// Map the engine's gate status onto the event vocabulary's verdict.
const fn verdict(status: ModuleStatus) -> CoverageVerdict {
    match status {
        ModuleStatus::Passed => CoverageVerdict::Passed,
        ModuleStatus::Failed => CoverageVerdict::Failed,
        ModuleStatus::Advisory => CoverageVerdict::Advisory,
        ModuleStatus::Excluded => CoverageVerdict::Excluded,
    }
}

#[cfg(test)]
mod tests {
    use toven_model::{CoverageMetric, CoverageVerdict, EcosystemId, Event, ModuleKey, ModuleRef};
    use toven_ports::Enforcement;
    use toven_testkit::RecordingReporter;

    use super::emit_verdicts;
    use crate::coverage::gate::{
        CoverageDimension, DimensionOutcome, ModuleCoverage, ModuleStatus,
    };
    use crate::coverage::metrics::CoverageMetrics;
    use crate::coverage::report::CoverageReport;

    fn key(name: &str) -> ModuleKey {
        ModuleKey::bare(ModuleRef::new(EcosystemId::new("rust").unwrap(), name).unwrap())
    }

    fn passed(name: &str) -> ModuleCoverage {
        ModuleCoverage {
            module: key(name),
            metrics: CoverageMetrics {
                line: 95.37,
                function: None,
                region: None,
                changed_line: None,
            },
            enforcement: Enforcement::Block,
            outcomes: vec![DimensionOutcome {
                dimension: CoverageDimension::Line,
                measured: 95.37,
                threshold: 90.0,
                passed: true,
            }],
            status: ModuleStatus::Passed,
        }
    }

    fn failed(name: &str) -> ModuleCoverage {
        ModuleCoverage {
            module: key(name),
            metrics: CoverageMetrics {
                line: 40.0,
                function: None,
                region: None,
                changed_line: None,
            },
            enforcement: Enforcement::Block,
            outcomes: vec![DimensionOutcome {
                dimension: CoverageDimension::Line,
                measured: 40.0,
                threshold: 90.0,
                passed: false,
            }],
            status: ModuleStatus::Failed,
        }
    }

    #[test]
    fn emits_one_ordered_verdict_per_module_including_a_failing_one() {
        let report = CoverageReport {
            modules: vec![passed("core"), failed("cli")],
            changed: false,
        };
        let mut reporter = RecordingReporter::new();
        emit_verdicts(&mut reporter, &report).expect("emits");

        // One record per module, in report order — a failing module still streams
        // its verdict (fail closed), it does not suppress the line.
        assert_eq!(reporter.len(), report.modules.len());
        match &reporter.events()[0] {
            Event::ModuleCoverageFinished {
                module,
                measurements,
                verdict,
            } => {
                assert_eq!(module, "rust:core");
                assert_eq!(*verdict, CoverageVerdict::Passed);
                assert_eq!(measurements.len(), 1);
                assert_eq!(measurements[0].metric, CoverageMetric::Line);
                assert_eq!(measurements[0].measured, 9537);
                assert_eq!(measurements[0].threshold, Some(9000));
                assert!(measurements[0].met);
            }
            other => panic!("expected a coverage verdict, got {other:?}"),
        }
        match &reporter.events()[1] {
            Event::ModuleCoverageFinished {
                module, verdict, ..
            } => {
                assert_eq!(module, "rust:cli");
                assert_eq!(*verdict, CoverageVerdict::Failed);
            }
            other => panic!("expected a coverage verdict, got {other:?}"),
        }
    }

    #[test]
    fn a_measured_but_ungated_dimension_carries_no_threshold_and_is_met() {
        let mut module = passed("core");
        // Measured function coverage with no gate outcome (no configured floor).
        module.metrics.function = Some(88.0);
        let report = CoverageReport {
            modules: vec![module],
            changed: false,
        };
        let mut reporter = RecordingReporter::new();
        emit_verdicts(&mut reporter, &report).expect("emits");

        match &reporter.events()[0] {
            Event::ModuleCoverageFinished { measurements, .. } => {
                let function = measurements
                    .iter()
                    .find(|m| m.metric == CoverageMetric::Function)
                    .expect("function measurement present");
                assert_eq!(function.measured, 8800);
                assert_eq!(function.threshold, None);
                assert!(function.met, "an ungated dimension is always met");
            }
            other => panic!("expected a coverage verdict, got {other:?}"),
        }
    }
}
