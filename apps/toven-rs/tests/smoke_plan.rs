//! Standalone `toven-rs` PLAN-cut smokes: `plan <task>`, `run <task>
//! --dry-run`, and `--output jsonl`. Human summaries render to **stderr**; the
//! JSONL stream renders to **stdout**. No APPLY side effects.

mod common;

use common::{repo, toven_rs, toven_rs_ok};

#[test]
fn plan_reports_units_waves_and_zero_ran() {
    let sample = repo("rust/workspace-linear");
    toven_rs_ok(&sample, &["plan", "build"])
        .expect_stderr_contains("plan:")
        .expect_stderr_contains("unit")
        .expect_stderr_contains("wave")
        .expect_stderr_contains("ran:  0");
}

#[test]
fn run_dry_run_does_not_apply() {
    let sample = repo("rust/workspace-linear");
    let out = toven_rs(&sample, &["run", "build", "--dry-run"]);
    out.expect_success().expect_stderr_contains("ran:  0");
    assert!(
        !out.stderr.contains("  ok "),
        "a dry run must not emit APPLY status lines"
    );
}

#[test]
fn jsonl_output_streams_valid_json_lines() {
    let sample = repo("rust/single");
    let out = toven_rs_ok(&sample, &["--output", "jsonl", "plan", "build"]);
    let first = out.stdout.lines().next().expect("at least one jsonl line");
    let value: serde_json::Value = serde_json::from_str(first).expect("first line parses");
    assert_eq!(value["event"], "run-started");
    for line in out.stdout.lines() {
        serde_json::from_str::<serde_json::Value>(line)
            .unwrap_or_else(|error| panic!("invalid jsonl ({error}): {line}"));
    }
}
