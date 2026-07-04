//! Standalone `toven-go` repo-driven smokes: discovery, PLAN, and APPLY against
//! real Go module fixtures, mirroring the Rust app's coverage. Go discovery
//! shells out to the `go` toolchain, so every test is gated on [`go_available`]
//! and skips (green) when no `go` is installed. Introspection projections render
//! to **stdout**; the human run reporter renders to **stderr**.

mod common;

use common::{go_available, repo, toven_go, toven_go_ok};

macro_rules! require_go {
    () => {
        if !go_available() {
            eprintln!("skipping: no `go` toolchain on PATH");
            return;
        }
    };
}

#[test]
fn modules_lists_the_single_module_baseline() {
    require_go!();
    let sample = repo("go/single");
    toven_go_ok(&sample, &["modules"]).expect_stdout_contains("go:app");
}

#[test]
fn graph_renders_the_go_dependency_edge() {
    require_go!();
    let sample = repo("go/multi-module");
    toven_go_ok(&sample, &["graph"])
        .expect_stdout_contains("go:app")
        .expect_stdout_contains("  -> go:core");
}

#[test]
fn explain_renders_the_planned_go_unit() {
    require_go!();
    // Regression lock for the flags.rs id-collision fix on the Go binary too.
    let sample = repo("go/multi-module");
    toven_go(&sample, &["explain", "go:core", "build"])
        .expect_success()
        .expect_stdout_contains("go:core")
        .expect_stdout_contains("task:")
        .expect_stdout_contains("argv:");
}

#[test]
fn plan_and_dry_run_stay_read_only() {
    require_go!();
    let sample = repo("go/multi-module");
    toven_go_ok(&sample, &["plan", "build"])
        .expect_stderr_contains("plan:")
        .expect_stderr_contains("ran:  0");
    let dry = toven_go(&sample, &["run", "build", "--dry-run"]);
    dry.expect_success().expect_stderr_contains("ran:  0");
    assert!(!dry.stderr.contains("  ok "), "a dry run must not apply");
}

#[test]
fn jsonl_output_streams_valid_json_lines() {
    require_go!();
    let sample = repo("go/single");
    let out = toven_go_ok(&sample, &["--output", "jsonl", "plan", "build"]);
    let first = out.stdout.lines().next().expect("a jsonl line");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(first).expect("parses")["event"],
        "run-started"
    );
}

#[test]
fn bare_task_dispatch_applies_go_modules() {
    require_go!();
    let sample = repo("go/multi-module");
    toven_go_ok(&sample, &["build"])
        .expect_stderr_contains("ok go:core#build")
        .expect_stderr_contains("ok go:app#build")
        .expect_stderr_contains("ran:  2");
}

#[test]
fn module_selection_limits_apply_to_the_selected_subgraph() {
    require_go!();
    let sample = repo("go/multi-module");
    toven_go_ok(&sample, &["--module", "go:core", "build"])
        .expect_stderr_contains("ok go:core#build")
        .expect_stderr_contains("ran:  1");
}

#[test]
fn cache_path_prints_an_absolute_path() {
    require_go!();
    let sample = repo("go/single");
    let out = toven_go_ok(&sample, &["cache", "path"]);
    assert!(
        std::path::Path::new(out.stdout.trim()).is_absolute(),
        "`cache path` should be absolute, got {:?}",
        out.stdout.trim()
    );
}
