//! Normalized coverage-profile model: the ecosystem-agnostic shape the lcov and
//! Go `-coverprofile` parsers fold their emitted output into.
//!
//! A profile is a set of per-file line/function/region tallies. Line coverage
//! is recorded per line (covered or not) so a changed-scope gate can restrict
//! the measurement to a subset of files. Function/region tallies are `Option` —
//! an ecosystem that cannot measure a dimension (Go emits statement/line
//! coverage only) leaves it `None`, and the gate skips any dimension a profile
//! did not measure rather than failing on a missing metric.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// A found/hit tally for one coverage dimension.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    /// Total instrumented items (lines, functions, or regions).
    pub found: u32,
    /// Items with at least one hit.
    pub hit: u32,
}

impl Counts {
    /// Fold another tally into this one.
    pub const fn add(&mut self, other: Self) {
        self.found += other.found;
        self.hit += other.hit;
    }

    /// The coverage percentage, or `100.0` when nothing was instrumented (a
    /// vacuously-covered file never drags an aggregate down).
    #[must_use]
    pub fn percentage(self) -> f64 {
        if self.found == 0 {
            100.0
        } else {
            f64::from(self.hit) / f64::from(self.found) * 100.0
        }
    }
}

/// Per-file coverage: line hit-map plus optional function/region tallies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileCoverage {
    /// The file path exactly as the tool emitted it (normalized at
    /// attribution).
    pub path: PathBuf,
    /// Per-line coverage: line number → whether it was hit at least once.
    pub lines: BTreeMap<u32, bool>,
    /// Function tally; `None` where the ecosystem does not measure functions.
    pub functions: Option<Counts>,
    /// Region/branch tally; `None` where the ecosystem does not measure
    /// regions.
    pub regions: Option<Counts>,
}

impl FileCoverage {
    /// A file with only a line hit-map (Go statement coverage).
    #[must_use]
    pub fn lines_only(path: impl Into<PathBuf>, lines: BTreeMap<u32, bool>) -> Self {
        Self {
            path: path.into(),
            lines,
            functions: None,
            regions: None,
        }
    }

    /// The file's line tally.
    #[must_use]
    pub fn line_counts(&self) -> Counts {
        let hit = self.lines.values().filter(|covered| **covered).count();
        Counts {
            found: u32::try_from(self.lines.len()).unwrap_or(u32::MAX),
            hit: u32::try_from(hit).unwrap_or(u32::MAX),
        }
    }

    /// Mark `line` covered (OR-merging repeated observations of one line).
    pub fn observe_line(&mut self, line: u32, covered: bool) {
        let entry = self.lines.entry(line).or_insert(false);
        *entry = *entry || covered;
    }
}

/// A parsed coverage profile: the per-file tallies emitted by one run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct CoverageProfile {
    /// Per-file coverage records.
    pub(super) files: Vec<FileCoverage>,
}

/// The coverage profile wire format, detected from a profile file's contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CoverageFormat {
    /// LCOV tracefile (`cargo llvm-cov --lcov`).
    Lcov,
    /// Go `-coverprofile` (`mode:` header + block records).
    GoProfile,
}

impl CoverageFormat {
    /// Detect the format from a profile file's leading content.
    ///
    /// A Go coverprofile always opens with a `mode:` line; anything else is
    /// treated as LCOV (the portable interchange format llvm-cov emits).
    #[must_use]
    pub(super) fn detect(contents: &str) -> Self {
        if contents.trim_start().starts_with("mode:") {
            Self::GoProfile
        } else {
            Self::Lcov
        }
    }
}
