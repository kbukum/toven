//! LCOV tracefile parser (`cargo llvm-cov --lcov`).
//!
//! Reads the subset of the LCOV grammar Toven gates on: `SF` (source file),
//! `DA` (per-line hit count), `FN`/`FNDA` (per-function hits), `BRDA`
//! (per-branch hits, Toven's region proxy), and the `FNF`/`FNH`/`BRF`/`BRH`
//! summaries used when a tracefile has no per-item records. Per-item records
//! let several runs of one file merge exactly. Unknown records are ignored so
//! a richer tracefile still parses. Region coverage maps to LCOV branch coverage,
//! the closest portable analogue llvm-cov emits in `--lcov` output.

use std::collections::BTreeMap;

use rskit_errors::{AppError, AppResult};

use super::profile::{CoverageFormat, CoverageProfile, FileCoverage, Tally};

/// Parse an LCOV tracefile into a normalized [`CoverageProfile`].
///
/// # Errors
/// Rejects a malformed `DA`/`FN`/`FNDA`/`BRDA` record or a summary whose
/// numeric fields do not parse.
pub(super) fn parse(contents: &str) -> AppResult<CoverageProfile> {
    let mut files = Vec::new();
    let mut current: Option<FileCoverage> = None;
    let mut functions = Tally::default();
    let mut regions = Tally::default();
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
            functions = Tally::default();
            regions = Tally::default();
            saw_functions = false;
            saw_regions = false;
        } else if let Some(record) = line.strip_prefix("DA:") {
            let file = current
                .as_mut()
                .ok_or_else(|| lcov_error("DA record before any SF record"))?;
            let (line_no, hits) = split_pair("DA", record)?;
            file.observe_line(line_no, hits > 0);
        } else if let Some(record) = line.strip_prefix("FNDA:") {
            let (hits, name) = record
                .split_once(',')
                .ok_or_else(|| lcov_error(&format!("malformed FNDA record '{record}'")))?;
            functions.observe(function_name("FNDA", name)?, parse_u32("FNDA", hits)? > 0);
            saw_functions = true;
        } else if let Some(record) = line.strip_prefix("FN:") {
            functions.observe(fn_record_name(record)?, false);
            saw_functions = true;
        } else if let Some(record) = line.strip_prefix("BRDA:") {
            let (branch, taken) = brda_record(record)?;
            regions.observe(branch, taken);
            saw_regions = true;
        } else if let Some(value) = line.strip_prefix("FNF:") {
            functions.set_found(parse_u32("FNF", value)?);
            saw_functions = true;
        } else if let Some(value) = line.strip_prefix("FNH:") {
            functions.set_hit(parse_u32("FNH", value)?);
            saw_functions = true;
        } else if let Some(value) = line.strip_prefix("BRF:") {
            regions.set_found(parse_u32("BRF", value)?);
            saw_regions = true;
        } else if let Some(value) = line.strip_prefix("BRH:") {
            regions.set_hit(parse_u32("BRH", value)?);
            saw_regions = true;
        } else if line == "end_of_record"
            && let Some(mut file) = current.take()
        {
            file.functions = saw_functions.then(|| std::mem::take(&mut functions));
            file.regions = saw_regions.then(|| std::mem::take(&mut regions));
            files.push(file);
        }
    }
    if let Some(mut file) = current.take() {
        file.functions = saw_functions.then_some(functions);
        file.regions = saw_regions.then_some(regions);
        files.push(file);
    }
    Ok(CoverageProfile {
        format: CoverageFormat::Lcov,
        producer: None,
        files,
    })
}

/// Split a `<line>,<hits>` record used by `DA`.
fn split_pair(record: &str, value: &str) -> AppResult<(u32, u32)> {
    let (left, right) = value
        .split_once(',')
        .ok_or_else(|| lcov_error(&format!("malformed {record} record '{value}'")))?;
    Ok((parse_u32(record, left)?, parse_u32(record, right)?))
}

/// The function name of an `FN:<line>[,<end line>],<name>` record. Names may
/// contain commas, so only leading numeric fields are stripped.
fn fn_record_name(record: &str) -> AppResult<&str> {
    let (line, rest) = record
        .split_once(',')
        .ok_or_else(|| lcov_error(&format!("malformed FN record '{record}'")))?;
    parse_u32("FN", line)?;
    let name = match rest.split_once(',') {
        Some((end, name)) if end.trim().parse::<u32>().is_ok() => name,
        _ => rest,
    };
    function_name("FN", name)
}

/// A non-empty function name.
fn function_name<'a>(record: &str, name: &'a str) -> AppResult<&'a str> {
    let name = name.trim();
    if name.is_empty() {
        return Err(lcov_error(&format!(
            "{record} record has an empty function name"
        )));
    }
    Ok(name)
}

/// The identity and covered state of a `BRDA:<line>,<block>,<branch>,<taken>`
/// record; `taken` is a hit count or `-` for a branch that never ran.
fn brda_record(record: &str) -> AppResult<(String, bool)> {
    let malformed = || lcov_error(&format!("malformed BRDA record '{record}'"));
    let (branch, taken) = record.rsplit_once(',').ok_or_else(malformed)?;
    let mut fields = branch.splitn(3, ',');
    let line = fields.next().ok_or_else(malformed)?;
    let block = fields.next().ok_or_else(malformed)?;
    let id = fields.next().ok_or_else(malformed)?;
    parse_u32("BRDA", line)?;
    if block.trim().is_empty() || id.trim().is_empty() {
        return Err(malformed());
    }
    let taken = match taken.trim() {
        "-" => false,
        count => parse_u32("BRDA", count)? > 0,
    };
    Ok((branch.trim().to_owned(), taken))
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
        assert_eq!(file.functions.as_ref().expect("functions").counts().hit, 1);
        assert_eq!(file.regions.as_ref().expect("regions").counts().found, 4);
    }

    #[test]
    fn per_item_records_identify_functions_and_branches() {
        let profile = parse(
            "SF:a.rs\n\
             FN:3,alpha\n\
             FN:9,14,beta,with,commas\n\
             FNDA:2,alpha\n\
             FNDA:0,beta,with,commas\n\
             FNF:2\n\
             FNH:1\n\
             BRDA:4,0,0,3\n\
             BRDA:4,0,1,-\n\
             BRDA:5,0,0,0\n\
             BRF:3\n\
             BRH:1\n\
             end_of_record\n",
        )
        .expect("parses");

        let file = &profile.files[0];
        let functions = file.functions.as_ref().expect("functions").counts();
        assert_eq!((functions.found, functions.hit), (2, 1));
        let regions = file.regions.as_ref().expect("regions").counts();
        assert_eq!((regions.found, regions.hit), (3, 1));
    }

    #[test]
    fn complementary_runs_merge_by_item_identity() {
        let run = |alpha: u32, beta: u32| {
            parse(&format!(
                "SF:a.rs\nFN:1,alpha\nFN:5,beta\nFNDA:{alpha},alpha\nFNDA:{beta},beta\n\
                 BRDA:2,0,0,{alpha}\nBRDA:2,0,1,{beta}\nend_of_record\n"
            ))
            .expect("parses")
            .files
            .remove(0)
        };
        let mut merged = run(1, 0);
        merged.merge(&run(0, 1));
        merged.merge(&run(1, 0));

        let functions = merged.functions.as_ref().expect("functions").counts();
        assert_eq!((functions.found, functions.hit), (2, 2));
        let regions = merged.regions.as_ref().expect("regions").counts();
        assert_eq!((regions.found, regions.hit), (2, 2));
    }

    #[test]
    fn runs_with_different_items_unite_the_item_sets() {
        let mut merged = parse("SF:a.rs\nFN:1,alpha\nFNDA:1,alpha\nend_of_record\n")
            .expect("parses")
            .files
            .remove(0);
        let other = parse("SF:a.rs\nFN:5,beta\nFNDA:0,beta\nend_of_record\n")
            .expect("parses")
            .files
            .remove(0);
        merged.merge(&other);
        let functions = merged.functions.as_ref().expect("functions").counts();
        assert_eq!((functions.found, functions.hit), (2, 1));
    }

    #[test]
    fn summary_only_runs_keep_the_larger_counts() {
        let run = |hit: u32| {
            parse(&format!("SF:a.rs\nFNF:4\nFNH:{hit}\nend_of_record\n"))
                .expect("parses")
                .files
                .remove(0)
        };
        let mut merged = run(1);
        merged.merge(&run(3));
        let functions = merged.functions.as_ref().expect("functions").counts();
        assert_eq!((functions.found, functions.hit), (4, 3));
    }

    #[test]
    fn rejects_malformed_item_records() {
        for record in [
            "FN:x,alpha",
            "FN:1",
            "FN:1,",
            "FNDA:x,alpha",
            "FNDA:1",
            "FNDA:1,",
            "BRDA:1,0,0",
            "BRDA:x,0,0,1",
            "BRDA:1,0,0,x",
            "BRDA:1,,0,1",
        ] {
            assert!(
                parse(&format!("SF:a.rs\n{record}\nend_of_record\n")).is_err(),
                "{record} must be rejected"
            );
        }
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
