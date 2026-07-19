//! Coverage metrics: fold a set of per-file tallies into per-dimension
//! percentages.
//!
//! `line` is always present; `function`/`region` are `Some` only when at least
//! one file measured that dimension, so a Go module (line-only) yields `None`
//! for both and the gate skips them. `changed_line` is the line percentage over
//! only the changed files, computed under `--changed`.

use std::collections::BTreeSet;
use std::path::PathBuf;

use super::profile::{Counts, FileCoverage};

/// The per-dimension coverage percentages for one module.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoverageMetrics {
    /// Absolute line-coverage percentage.
    pub line: f64,
    /// Function-coverage percentage; `None` where unmeasured.
    pub function: Option<f64>,
    /// Region-coverage percentage; `None` where unmeasured.
    pub region: Option<f64>,
    /// Line coverage over the changed files; `None` outside `--changed` or when
    /// no measured file changed.
    pub changed_line: Option<f64>,
}

impl CoverageMetrics {
    /// Compute the metrics for a module from its files.
    ///
    /// `changed` is the set of changed file paths under `--changed`; `None`
    /// leaves `changed_line` unset. Only files present in this module are
    /// folded.
    #[must_use]
    pub fn compute(files: &[&FileCoverage], changed: Option<&BTreeSet<PathBuf>>) -> Self {
        let mut lines = Counts::default();
        let mut functions = Counts::default();
        let mut regions = Counts::default();
        let mut saw_functions = false;
        let mut saw_regions = false;
        let mut changed_lines = Counts::default();
        let mut saw_changed = false;

        for file in files {
            lines.add(file.line_counts());
            if let Some(counts) = file.functions {
                functions.add(counts);
                saw_functions = true;
            }
            if let Some(counts) = file.regions {
                regions.add(counts);
                saw_regions = true;
            }
            if changed.is_some_and(|set| set.contains(&file.path)) {
                changed_lines.add(file.line_counts());
                saw_changed = true;
            }
        }

        Self {
            line: lines.percentage(),
            function: saw_functions.then(|| functions.percentage()),
            region: saw_regions.then(|| regions.percentage()),
            changed_line: saw_changed.then(|| changed_lines.percentage()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::CoverageMetrics;
    use crate::coverage::profile::{Counts, FileCoverage};

    fn file(path: &str, hit: u32, found: u32) -> FileCoverage {
        let mut lines = BTreeMap::new();
        for line in 1..=found {
            lines.insert(line, line <= hit);
        }
        FileCoverage {
            path: path.into(),
            lines,
            functions: None,
            regions: None,
        }
    }

    #[test]
    fn line_percentage_aggregates_across_files() {
        let a = file("a.rs", 8, 10);
        let b = file("b.rs", 2, 10);
        let metrics = CoverageMetrics::compute(&[&a, &b], None);
        assert!((metrics.line - 50.0).abs() < 1e-9);
        assert!(metrics.function.is_none());
        assert!(metrics.changed_line.is_none());
    }

    #[test]
    fn changed_line_folds_only_changed_files() {
        let a = file("a.rs", 9, 10);
        let b = file("b.rs", 1, 10);
        let changed: BTreeSet<_> = std::iter::once(std::path::PathBuf::from("b.rs")).collect();
        let metrics = CoverageMetrics::compute(&[&a, &b], Some(&changed));
        assert!((metrics.changed_line.expect("changed") - 10.0).abs() < 1e-9);
    }

    #[test]
    fn function_dimension_present_when_measured() {
        let mut a = file("a.rs", 5, 10);
        a.functions = Some(Counts { found: 4, hit: 3 });
        let metrics = CoverageMetrics::compute(&[&a], None);
        assert!((metrics.function.expect("function") - 75.0).abs() < 1e-9);
    }
}
