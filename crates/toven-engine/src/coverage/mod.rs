//! First-class coverage: aggregate the ecosystem coverage tool's emitted
//! profiles per module and gate them against the resolved `[…coverage]`
//! thresholds.
//!
//! The recognized coverage task measures (llvm-cov lcov / Go `-coverprofile`);
//! this module aggregates and decides the verdict. [`coverage_report`] is the
//! entry the CLI `coverage` verb calls after running the task; the submodules
//! hold the profile model + parsers (`profile`/`lcov`/`goprofile`), the
//! per-dimension `metrics`, the resolved `settings`, the `gate`, and the
//! aggregated `report`.

mod aggregate;
mod entry;
mod gate;
mod goprofile;
mod lcov;
mod metrics;
mod profile;
mod read;
mod report;
mod settings;
mod stream;

pub use entry::coverage_report;
pub use gate::{CoverageDimension, DimensionOutcome, ModuleCoverage, ModuleStatus};
pub use metrics::CoverageMetrics;
pub use read::COVERAGE_DIR;
pub use report::{CoverageReport, ReportTally};
pub use settings::{CoverageOverrides, ResolvedCoverageSettings};
