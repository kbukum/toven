//! Integration tests for the `toven release` lifecycle subcommand.
//! Exercises status, plan, readiness, dry-run publish, and SBOM/depgraphs.

mod common;

use common::{repo, toven_ok};

#[test]
fn release_plan_renders_release_plan_table_or_jsonl() {
    let sample = repo("rust/single");
    let out = toven_ok(&sample, &["release", "plan"]);
    out.expect_stdout_contains("Release plan")
        .expect_stdout_contains("app");

    // Test JSONL output
    let out_jsonl = toven_ok(&sample, &["--output", "jsonl", "release", "plan"]);
    out_jsonl.expect_stdout_contains("\"module\":\"rust:app\"");
}

#[test]
fn release_status_renders_release_status_table_or_jsonl() {
    let sample = repo("rust/single");
    let out = toven_ok(&sample, &["release", "status"]);
    out.expect_stdout_contains("Release status")
        .expect_stdout_contains("app");

    // Test JSONL output
    let out_jsonl = toven_ok(&sample, &["--output", "jsonl", "release", "status"]);
    out_jsonl.expect_stdout_contains("\"declared_version\":\"0.1.0\"");
}

#[test]
fn release_readiness_runs_guards_and_verifies_clean() {
    let sample = repo("rust/single");
    let out = toven_ok(&sample, &["release", "readiness"]);
    out.expect_stdout_contains("Release readiness")
        .expect_stdout_contains("verdict:  go");
}

#[test]
fn release_dry_run_publish_rehearses_correctly() {
    let sample = repo("rust/single");
    let out = toven_ok(&sample, &["release", "publish", "--dry-run", "--offline"]);
    out.expect_stdout_contains("Release rehearsal")
        .expect_stdout_contains("would-publish");
}
