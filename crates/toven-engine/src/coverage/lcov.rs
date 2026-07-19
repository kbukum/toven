//! LCOV tracefile parser (`cargo llvm-cov --lcov`).
//!
//! Reads the subset of the LCOV grammar Toven gates on: `SF` (source file),
//! `DA` (per-line hit count), `FNF`/`FNH` (function found/hit), and `BRF`/`BRH`
//! (branch found/hit, Toven's region proxy). Unknown records are ignored so a
//! richer tracefile still parses. Region coverage maps to LCOV branch coverage,
//! the closest portable analogue llvm-cov emits in `--lcov` output.

use std::collections::BTreeMap;

use rskit_errors::{AppError, AppResult};

use super::profile::{Counts, CoverageProfile, FileCoverage};

/// Parse an LCOV tracefile into a normalized [`CoverageProfile`].
///
/// # Errors
/// Rejects a malformed `DA:`/`FNF:`/`BRF:` record whose numeric fields do not
/// parse.
pub(super) fn parse(contents: &str) -> AppResult<CoverageProfile> {
    let mut files = Vec::new();
    let mut current: Option<FileCoverage> = None;
    let mut functions = Counts::default();
    let mut regions = Counts::default();
    let mut saw_functions = false;
    let mut saw_regions = false;

    for line in contents.lines() {
        let line = line.trim();
        if let Some(path) = line.strip_prefix("SF:") {
            current = Some(FileCoverage {
                path: path.trim().into(),
                lines: BTreeMap::new(),
                functions: None,
                regions: None,
            });
            functions = Counts::default();
            regions = Counts::default();
            saw_functions = false;
            saw_regions = false;
        } else if let Some(record) = line.strip_prefix("DA:") {
            let file = current
                .as_mut()
                .ok_or_else(|| lcov_error("DA record before any SF record"))?;
            let (line_no, hits) = split_pair("DA", record)?;
            file.observe_line(line_no, hits > 0);
        } else if let Some(value) = line.strip_prefix("FNF:") {
            functions.found = parse_u32("FNF", value)?;
            saw_functions = true;
        } else if let Some(value) = line.strip_prefix("FNH:") {
            functions.hit = parse_u32("FNH", value)?;
            saw_functions = true;
        } else if let Some(value) = line.strip_prefix("BRF:") {
            regions.found = parse_u32("BRF", value)?;
            saw_regions = true;
        } else if let Some(value) = line.strip_prefix("BRH:") {
            regions.hit = parse_u32("BRH", value)?;
            saw_regions = true;
        } else if line == "end_of_record"
            && let Some(mut file) = current.take()
        {
            file.functions = saw_functions.then_some(functions);
            file.regions = saw_regions.then_some(regions);
            files.push(file);
        }
    }
    if let Some(mut file) = current.take() {
        file.functions = saw_functions.then_some(functions);
        file.regions = saw_regions.then_some(regions);
        files.push(file);
    }
    Ok(CoverageProfile { files })
}

/// Split a `<line>,<hits>` record used by `DA`.
fn split_pair(record: &str, value: &str) -> AppResult<(u32, u32)> {
    let (left, right) = value
        .split_once(',')
        .ok_or_else(|| lcov_error(&format!("malformed {record} record '{value}'")))?;
    Ok((parse_u32(record, left)?, parse_u32(record, right)?))
}

/// Parse a `u32` field, citing the record it came from on failure.
fn parse_u32(record: &str, value: &str) -> AppResult<u32> {
    value
        .trim()
        .parse()
        .map_err(|_| lcov_error(&format!("{record} field '{value}' is not a number")))
}

/// A typed parse error for a malformed tracefile.
fn lcov_error(detail: &str) -> AppError {
    AppError::invalid_input("coverage.lcov", format!("invalid LCOV profile: {detail}"))
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn parses_line_function_and_region_tallies() {
        let profile = parse(
            "SF:crates/toven-process/src/lib.rs\n\
             DA:1,5\n\
             DA:2,0\n\
             DA:3,2\n\
             FNF:2\n\
             FNH:1\n\
             BRF:4\n\
             BRH:3\n\
             end_of_record\n",
        )
        .expect("parses");

        assert_eq!(profile.files.len(), 1);
        let file = &profile.files[0];
        assert_eq!(file.line_counts().found, 3);
        assert_eq!(file.line_counts().hit, 2);
        assert_eq!(file.functions.expect("functions").hit, 1);
        assert_eq!(file.regions.expect("regions").found, 4);
    }

    #[test]
    fn leaves_unmeasured_dimensions_none() {
        let profile = parse("SF:a.rs\nDA:1,1\nend_of_record\n").expect("parses");
        assert!(profile.files[0].functions.is_none());
        assert!(profile.files[0].regions.is_none());
    }

    #[test]
    fn rejects_malformed_da_record() {
        assert!(parse("SF:a.rs\nDA:notaline\nend_of_record\n").is_err());
    }
}
