//! Go `-coverprofile` parser.
//!
//! Reads the coverprofile grammar `go test -coverprofile` emits: a `mode:`
//! header followed by one block record per instrumented statement span,
//! `<file>:<startLine>.<col>,<endLine>.<col> <numStmts> <count>`. Go measures
//! statement (line) coverage only, so function/region tallies stay `None` and
//! the gate skips those dimensions for Go modules. Each statement span marks its
//! covered lines, OR-merged across overlapping spans.

use std::collections::BTreeMap;

use rskit_errors::{AppError, AppResult};

use super::profile::{CoverageProfile, FileCoverage};

/// Parse a Go coverprofile into a normalized [`CoverageProfile`].
///
/// # Errors
/// Rejects a record whose position/statement/count fields do not parse.
pub(super) fn parse(contents: &str) -> AppResult<CoverageProfile> {
    let mut files: BTreeMap<String, FileCoverage> = BTreeMap::new();

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("mode:") {
            continue;
        }
        let record = parse_record(line)?;
        let file = files
            .entry(record.file.clone())
            .or_insert_with(|| FileCoverage::lines_only(&record.file, BTreeMap::new()));
        for line_no in record.start_line..=record.end_line {
            file.observe_line(line_no, record.count > 0);
        }
    }

    Ok(CoverageProfile {
        files: files.into_values().collect(),
    })
}

/// One parsed coverprofile block record.
struct Record {
    file: String,
    start_line: u32,
    end_line: u32,
    count: u64,
}

/// Upper bound on the line span of a single coverprofile record. A real Go
/// source file never approaches this, so a wider span means a malformed or
/// hostile profile — rejecting it keeps the per-line expansion bounded.
const MAX_SPAN_LINES: u32 = 1_000_000;

/// Parse `<file>:<sl>.<sc>,<el>.<ec> <numStmts> <count>`.
fn parse_record(line: &str) -> AppResult<Record> {
    let (file, rest) = line
        .rsplit_once(':')
        .ok_or_else(|| go_error(&format!("missing file separator in '{line}'")))?;
    let mut fields = rest.split_whitespace();
    let span = fields
        .next()
        .ok_or_else(|| go_error(&format!("missing span in '{line}'")))?;
    let _num_stmts = fields
        .next()
        .ok_or_else(|| go_error(&format!("missing statement count in '{line}'")))?;
    let count = fields
        .next()
        .ok_or_else(|| go_error(&format!("missing hit count in '{line}'")))?;

    let (start, end) = span
        .split_once(',')
        .ok_or_else(|| go_error(&format!("malformed span '{span}'")))?;
    let start_line = parse_position(start)?;
    let end_line = parse_position(end)?;
    if end_line < start_line {
        return Err(go_error(&format!(
            "span end line {end_line} precedes start line {start_line}"
        )));
    }
    if end_line - start_line >= MAX_SPAN_LINES {
        return Err(go_error(&format!(
            "span of {} lines exceeds the {MAX_SPAN_LINES}-line limit",
            u64::from(end_line - start_line) + 1
        )));
    }
    Ok(Record {
        file: file.to_string(),
        start_line,
        end_line,
        count: count
            .parse()
            .map_err(|_| go_error(&format!("hit count '{count}' is not a number")))?,
    })
}

/// Parse the line component of a `<line>.<col>` position.
fn parse_position(value: &str) -> AppResult<u32> {
    let line = value.split_once('.').map_or(value, |(line, _)| line);
    line.parse()
        .map_err(|_| go_error(&format!("position '{value}' is not a line.col pair")))
}

/// A typed parse error for a malformed coverprofile.
fn go_error(detail: &str) -> AppError {
    AppError::invalid_input(
        "coverage.goprofile",
        format!("invalid Go coverprofile: {detail}"),
    )
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn parses_statement_spans_into_line_coverage() {
        let profile = parse(
            "mode: set\n\
             toven/internal/foo/bar.go:10.20,12.2 2 1\n\
             toven/internal/foo/bar.go:15.2,16.3 1 0\n",
        )
        .expect("parses");

        assert_eq!(profile.files.len(), 1);
        let file = &profile.files[0];
        // lines 10,11,12 covered; 15,16 uncovered.
        assert_eq!(file.line_counts().found, 5);
        assert_eq!(file.line_counts().hit, 3);
        assert!(file.functions.is_none());
    }

    #[test]
    fn overlapping_spans_or_merge_coverage() {
        let profile = parse(
            "mode: count\n\
             a.go:1.1,2.2 1 0\n\
             a.go:2.1,3.2 1 5\n",
        )
        .expect("parses");
        // line 2 is covered by the second span despite the first marking it 0.
        assert!(profile.files[0].lines[&2]);
    }

    #[test]
    fn rejects_malformed_record() {
        assert!(parse("mode: set\ngarbage line without colon\n").is_err());
    }

    #[test]
    fn rejects_oversized_span() {
        // A tiny record claiming a multi-billion-line span must not drive an
        // unbounded per-line expansion — it is rejected as malformed.
        let error =
            parse("mode: set\nf.go:1.1,4000000000.2 1 1\n").expect_err("oversized span rejected");
        assert!(error.to_string().contains("exceeds"), "{error}");
    }

    #[test]
    fn rejects_inverted_span() {
        assert!(parse("mode: set\nf.go:10.1,2.2 1 1\n").is_err());
    }
}
