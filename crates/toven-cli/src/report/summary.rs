//! Runner test-count summary parsing for the tiles verdict line.
//!
//! A succeeding `test` unit collapses to a single verdict line, and Toven — which
//! owns runner semantics — folds the runner's own count summary into it (e.g.
//! `ok rust:core#test · 987 passed, 3 skipped`). This module scans a unit's raw
//! output for that summary as bytes arrive, so no full transcript is buffered:
//! only the current partial line and the running tally are kept.
//!
//! Two runner shapes are recognized after stripping ANSI styling:
//! - `cargo-nextest`: `Summary [ … ] N tests run: N passed, M skipped` (one final
//!   grand total, so it overwrites the tally).
//! - `cargo test`: `test result: ok. N passed; M failed; K ignored; …` (one line
//!   per test binary, so the counts accumulate).

use std::fmt;

/// A parsed test-count summary for a unit's verdict tail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct RunSummary {
    /// Tests that passed.
    passed: usize,
    /// Tests skipped (nextest `skipped`, or `cargo test` `ignored`).
    skipped: usize,
}

impl fmt::Display for RunSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.skipped == 0 {
            write!(f, "{} passed", self.passed)
        } else {
            write!(f, "{} passed, {} skipped", self.passed, self.skipped)
        }
    }
}

/// Scans a unit's raw output stream for its runner test-count summary.
///
/// Feed raw output chunks with [`observe`](Self::observe) as they arrive; take
/// the folded [`RunSummary`] with [`summary`](Self::summary) once the unit ends.
#[derive(Debug, Default)]
pub(crate) struct SummaryScanner {
    pending: String,
    summary: Option<RunSummary>,
}

impl SummaryScanner {
    /// Feed a raw output chunk, updating the running tally line by line.
    pub(crate) fn observe(&mut self, bytes: &[u8]) {
        self.pending.push_str(&String::from_utf8_lossy(bytes));
        while let Some(newline) = self.pending.find('\n') {
            let line: String = self.pending.drain(..=newline).collect();
            self.scan_line(line.trim_end_matches(['\n', '\r']));
        }
    }

    /// The folded summary, if any count line was seen.
    pub(crate) const fn summary(&self) -> Option<RunSummary> {
        self.summary
    }

    fn scan_line(&mut self, line: &str) {
        let plain = strip_ansi(line);
        if let Some(counts) = parse_nextest(&plain) {
            // The nextest line is the grand total, so it replaces the tally.
            self.summary = Some(counts);
        } else if let Some(counts) = parse_cargo_test(&plain) {
            // Each cargo-test binary reports its own line, so counts accumulate.
            let total = self.summary.get_or_insert_with(RunSummary::default);
            total.passed += counts.passed;
            total.skipped += counts.skipped;
        }
    }
}

/// Strip ANSI escape sequences so digits inside SGR codes (e.g. `\x1b[32m`) do
/// not pollute the count parsing.
fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\x1b' {
            out.push(ch);
            continue;
        }
        // Drop a CSI (`ESC [ … final`) or, defensively, one trailing byte.
        if chars.peek() == Some(&'[') {
            chars.next();
            while let Some(&next) = chars.peek() {
                chars.next();
                if ('@'..='~').contains(&next) {
                    break;
                }
            }
        } else {
            chars.next();
        }
    }
    out
}

/// Parse a `cargo-nextest` grand-total line: `… N tests run: N passed, M skipped`.
fn parse_nextest(line: &str) -> Option<RunSummary> {
    let tail = line.split("tests run:").nth(1)?;
    Some(RunSummary {
        passed: count_before(tail, "passed")?,
        skipped: count_before(tail, "skipped").unwrap_or(0),
    })
}

/// Parse a `cargo test` binary line: `test result: ok. N passed; M failed; …`.
fn parse_cargo_test(line: &str) -> Option<RunSummary> {
    let tail = line.strip_prefix("test result:")?;
    Some(RunSummary {
        passed: count_before(tail, "passed")?,
        skipped: count_before(tail, "ignored").unwrap_or(0),
    })
}

/// The integer token immediately preceding `word` in `text`, if present.
fn count_before(text: &str, word: &str) -> Option<usize> {
    let head = text.split(word).next()?;
    head.rsplit(|c: char| !c.is_ascii_digit())
        .find(|token| !token.is_empty())
        .and_then(|token| token.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::{RunSummary, SummaryScanner};

    fn scan(chunks: &[&[u8]]) -> Option<RunSummary> {
        let mut scanner = SummaryScanner::default();
        for chunk in chunks {
            scanner.observe(chunk);
        }
        scanner.summary()
    }

    #[test]
    fn folds_a_nextest_summary_line() {
        let summary = scan(&[b"     Summary [   1.23s] 987 tests run: 987 passed, 3 skipped\n"])
            .expect("summary");
        assert_eq!(summary.to_string(), "987 passed, 3 skipped");
    }

    #[test]
    fn nextest_summary_omits_zero_skipped() {
        let summary = scan(&[b"   Summary [0.00s] 5 tests run: 5 passed, 0 skipped\n"]).unwrap();
        assert_eq!(summary.to_string(), "5 passed");
    }

    #[test]
    fn nextest_zero_tests_is_a_passing_summary() {
        let summary = scan(&[b"   Summary [0.00s] 0 tests run: 0 passed, 0 skipped\n"]).unwrap();
        assert_eq!(summary.to_string(), "0 passed");
    }

    #[test]
    fn accumulates_cargo_test_binary_lines() {
        let summary = scan(&[
            b"test result: ok. 3 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out\n",
            b"test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n",
        ])
        .expect("summary");
        assert_eq!(summary.to_string(), "5 passed, 1 skipped");
    }

    #[test]
    fn ignores_ansi_styling_around_counts() {
        let summary =
            scan(&[b"\x1b[32m   Summary\x1b[0m [0.1s] 42 tests run: 42 passed, 0 skipped\n"])
                .unwrap();
        assert_eq!(summary.to_string(), "42 passed");
    }

    #[test]
    fn reassembles_a_summary_split_across_chunks() {
        let summary = scan(&[b"Summary [0.1s] 7 tests ru", b"n: 7 passed, 1 skipped\n"]).unwrap();
        assert_eq!(summary.to_string(), "7 passed, 1 skipped");
    }

    #[test]
    fn no_summary_when_no_count_line_is_seen() {
        assert!(scan(&[b"Compiling foo v0.1.0\nRunning tests\n"]).is_none());
    }

    #[test]
    fn unterminated_summary_line_is_not_scanned_until_newline() {
        let mut scanner = SummaryScanner::default();
        scanner.observe(b"Summary [0.1s] 9 tests run: 9 passed, 0 skipped");
        assert!(scanner.summary().is_none());
        scanner.observe(b"\n");
        assert_eq!(scanner.summary().unwrap().to_string(), "9 passed");
    }
}
