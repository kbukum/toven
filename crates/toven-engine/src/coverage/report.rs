//! The coverage report: the per-module verdicts and the overall gate outcome
//! the CLI renders and exits on.

use super::gate::{ModuleCoverage, ModuleStatus};

/// The aggregated coverage verdict for a run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CoverageReport {
    /// Per-module verdicts, in stable module order.
    pub modules: Vec<ModuleCoverage>,
    /// Whether the run measured a changed scope (`--changed`).
    pub changed: bool,
}

impl CoverageReport {
    /// Whether the overall gate passes: no module failed under `block`.
    #[must_use]
    pub fn gate_passed(&self) -> bool {
        !self.modules.iter().any(|module| module.status.is_failure())
    }

    /// The modules that failed the gate closed.
    pub fn failures(&self) -> impl Iterator<Item = &ModuleCoverage> {
        self.modules
            .iter()
            .filter(|module| module.status.is_failure())
    }

    /// The count of modules in each status, for a summary line.
    #[must_use]
    pub fn tally(&self) -> ReportTally {
        let mut tally = ReportTally::default();
        for module in &self.modules {
            match module.status {
                ModuleStatus::Passed => tally.passed += 1,
                ModuleStatus::Failed => tally.failed += 1,
                ModuleStatus::Advisory => tally.advisory += 1,
                ModuleStatus::Excluded => tally.excluded += 1,
            }
        }
        tally
    }
}

/// Per-status module counts for the summary line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReportTally {
    /// Modules that met every gated floor.
    pub passed: usize,
    /// Modules that failed the gate closed.
    pub failed: usize,
    /// Modules below a floor but only advised.
    pub advisory: usize,
    /// Modules measured but excluded from gating.
    pub excluded: usize,
}
