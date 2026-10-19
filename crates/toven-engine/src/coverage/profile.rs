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

/// Coverage of one item dimension (functions or regions) in a file.
///
/// Items are keyed by their identity when the tool reports per-item records
/// (LCOV `FN`/`FNDA`, `BRDA`), so measurements from several runs merge exactly:
/// the item set is united and an item is covered when any run covered it.
/// Without per-item records only the tool's summary count is known.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tally {
    items: BTreeMap<String, bool>,
    summary: Counts,
}

impl Tally {
    /// A tally known only by its summary count.
    #[must_use]
    pub const fn from_counts(summary: Counts) -> Self {
        Self {
            items: BTreeMap::new(),
            summary,
        }
    }

    /// Record one observation of `item` (OR-merging repeated observations).
    pub fn observe(&mut self, item: impl Into<String>, covered: bool) {
        let entry = self.items.entry(item.into()).or_insert(false);
        *entry = *entry || covered;
    }

    /// Set the tool's summary count of instrumented items.
    pub const fn set_found(&mut self, found: u32) {
        self.summary.found = found;
    }

    /// Set the tool's summary count of covered items.
    pub const fn set_hit(&mut self, hit: u32) {
        self.summary.hit = hit;
    }

    /// The found/hit tally: from per-item records when present, else the
    /// summary.
    #[must_use]
    pub fn counts(&self) -> Counts {
        if self.items.is_empty() {
            return self.summary;
        }
        let hit = self.items.values().filter(|covered| **covered).count();
        Counts {
            found: u32::try_from(self.items.len()).unwrap_or(u32::MAX),
            hit: u32::try_from(hit).unwrap_or(u32::MAX),
        }
    }

    /// Fold another measurement of the same file's items into this one.
    ///
    /// With per-item records on both sides the items are united exactly.
    /// Otherwise items cannot be matched, so the larger of each count is kept:
    /// never double counted, at worst under-reported.
    pub fn merge(&mut self, other: &Self) {
        if !self.items.is_empty() && !other.items.is_empty() {
            for (item, &covered) in &other.items {
                self.observe(item.clone(), covered);
            }
            return;
        }
        let (left, right) = (self.counts(), other.counts());
        *self = Self::from_counts(Counts {
            found: left.found.max(right.found),
            hit: left.hit.max(right.hit),
        });
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
    pub functions: Option<Tally>,
    /// Region/branch tally; `None` where the ecosystem does not measure
    /// regions.
    pub regions: Option<Tally>,
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

    /// Fold another measurement of the same file into this one. Lines are
    /// OR-merged; function and region tallies merge per [`Tally::merge`].
    pub fn merge(&mut self, other: &Self) {
        for (&line, &covered) in &other.lines {
            self.observe_line(line, covered);
        }
        merge_tally(&mut self.functions, other.functions.as_ref());
        merge_tally(&mut self.regions, other.regions.as_ref());
    }
}

fn merge_tally(left: &mut Option<Tally>, right: Option<&Tally>) {
    match (left.as_mut(), right) {
        (Some(left), Some(right)) => left.merge(right),
        (None, Some(right)) => *left = Some(right.clone()),
        (_, None) => {}
    }
}

/// A parsed coverage profile: the per-file tallies emitted by one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CoverageProfile {
    /// The wire format the profile was parsed from. Attribution uses it to
    /// credit Go profiles only to Go modules and LCOV only to the others.
    pub(super) format: CoverageFormat,
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
