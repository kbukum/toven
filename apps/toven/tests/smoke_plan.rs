//! Umbrella `toven` PLAN-cut smokes: `plan <task>`, `run <task> --dry-run`, and
//! the `--output jsonl` event stream. The human run reporter renders to
//! **stderr**; the JSONL projection renders to **stdout**. All stay read-only —
//! no APPLY side effects.

mod common;

use common::{repo, toven, toven_ok};

#[test]
fn plan_reports_units_waves_and_zero_ran() {
    let sample = repo("cross-ecosystem/umbrella");
    let out = toven_ok(&sample, &["plan", "build"]);
    // Summary lines are on stderr (the human reporter stream).
    out.expect_stderr_contains("plan:")
        .expect_stderr_contains("units in")
        .expect_stderr_contains("ran:  0");
}

#[test]
fn run_dry_run_matches_the_plan_cut_without_applying() {
    let sample = repo("rust/multi-module");
    let out = toven(&sample, &["run", "build", "--dry-run"]);
    out.expect_success()
        .expect_stderr_contains("plan:")
        .expect_stderr_contains("ran:  0");
    // No APPLY status lines are emitted for a dry run.
    assert!(
        !out.stderr.contains("  ok "),
        "a dry run must not emit APPLY status lines"
    );
}

#[test]
fn jsonl_output_streams_a_run_started_event_first() {
    let sample = repo("cross-ecosystem/umbrella");
    let out = toven_ok(&sample, &["--output", "jsonl", "plan", "build"]);
    let first = out
        .stdout
        .lines()
        .next()
        .expect("jsonl output has at least one line");
    let value: serde_json::Value =
        serde_json::from_str(first).expect("the first jsonl line parses as JSON");
    assert_eq!(
        value["event"], "run-started",
        "first event is run-started: {first}"
    );
    assert_eq!(value["project"], "umbrella-multi-eco");
    // Every emitted line is a standalone JSON object (valid JSON-lines).
    for line in out.stdout.lines() {
        serde_json::from_str::<serde_json::Value>(line)
            .unwrap_or_else(|error| panic!("jsonl line is not valid JSON ({error}): {line}"));
    }
}

#[test]
fn jsonl_plan_stream_matches_the_deterministic_snapshot() {
    // The harness pins the wall clock (see `toven_testkit::smoke::CLOCK_EPOCH_ENV`),
    // so the machine-readable Event stream has no non-deterministic field left:
    // no timestamps, paths, or hashes, and a fixed `run_id`. That makes the whole
    // `jsonl` projection snapshot-stable, so this locks it byte-for-byte instead
    // of probing individual substrings — any unintended change to the emitted
    // event vocabulary or ordering fails here.
    let sample = repo("rust/single");
    let expected = concat!(
        r#"{"event":"run-started","run_id":"run-1700000000","intent":"build","project":"single-rust"}"#,
        "\n",
        r#"{"event":"phase-started","phase":"configure"}"#,
        "\n",
        r#"{"event":"phase-finished","phase":"configure"}"#,
        "\n",
        r#"{"event":"phase-started","phase":"discover"}"#,
        "\n",
        r#"{"event":"phase-finished","phase":"discover"}"#,
        "\n",
        r#"{"event":"phase-started","phase":"graph"}"#,
        "\n",
        r#"{"event":"phase-finished","phase":"graph"}"#,
        "\n",
        r#"{"event":"phase-started","phase":"affected"}"#,
        "\n",
        r#"{"event":"phase-finished","phase":"affected"}"#,
        "\n",
        r#"{"event":"phase-started","phase":"toolchain"}"#,
        "\n",
        r#"{"event":"phase-finished","phase":"toolchain"}"#,
        "\n",
        r#"{"event":"phase-started","phase":"schedule"}"#,
        "\n",
        r#"{"event":"cache-decided","unit_id":"rust@rust#build","verdict":"miss"}"#,
        "\n",
        r#"{"event":"phase-finished","phase":"schedule"}"#,
        "\n",
        r#"{"event":"plan-prepared","waves":1,"units":1}"#,
        "\n",
        r#"{"event":"run-finished","summary":{"planned_units":1,"ran_units":0,"cached_units":0,"failed_units":0,"blocked_units":0,"cancelled_units":0,"failed_readiness_units":0,"timed_out_units":0,"cache_hits":0,"cache_misses":1,"cache_disabled":0,"cache_forced":0,"dropped_output_chunks":0}}"#,
        "\n",
    );
    toven_ok(&sample, &["--output", "jsonl", "plan", "build"]).expect_stdout_eq(expected);
}
