//! Umbrella `toven` APPLY smokes: real subprocess execution across the bundled
//! ecosystems. Rust and `command` APPLY always run (their toolchains are the
//! test toolchain / a POSIX `false`/`true`); the Go and cross-ecosystem APPLY
//! are gated on a `go` toolchain being present so a runner without Go stays
//! green. APPLY status + summary lines render to **stderr**.

mod common;

use common::{repo, toven, toven_ok};
use toven_testkit::program_on_path;

#[test]
fn bare_task_dispatch_applies_a_rust_workspace() {
    let sample = repo("rust/multi-module");
    let out = toven_ok(&sample, &["check"]);
    out.expect_stderr_contains("  ok ")
        .expect_stderr_contains("ran:  1");
}

#[test]
fn argv_passthrough_splices_into_the_tool_verbatim() {
    // `format -- --check` proves the `--` tail reaches the tool (cargo fmt
    // `--check`) unchanged and stays non-mutating; the fixture is rustfmt-clean.
    let sample = repo("rust/multi-module");
    toven_ok(&sample, &["format", "--", "--check"]).expect_stderr_contains("  ok ");
}

#[test]
fn failing_task_propagates_a_nonzero_exit() {
    let sample = repo("command/failing-task");
    // `build` (`true`) is green; `check` (`false`) fails and is rendered.
    toven_ok(&sample, &["build"]);
    toven(&sample, &["check"])
        .expect_code(1)
        .expect_stderr_contains("failed command:boom#check")
        .expect_stderr_contains("failed:  1");
}

#[test]
fn fail_fast_stops_on_the_first_failure_with_a_nonzero_exit() {
    let sample = repo("command/failing-task");
    toven(&sample, &["--fail-fast", "check"])
        .expect_code(1)
        .expect_stderr_contains("failed:  1");
}

#[test]
fn module_selection_limits_apply_to_the_selected_subgraph() {
    if !program_on_path("go") {
        eprintln!("skipping: no `go` toolchain on PATH");
        return;
    }
    // Go emits per-module units, so `--module go:core` reduces the plan to one.
    let sample = repo("go/multi-module");
    toven_ok(&sample, &["--module", "go:core", "build"])
        .expect_stderr_contains("ok go:core#build")
        .expect_stderr_contains("ran:  1");
}

#[test]
fn multi_ecosystem_apply_runs_every_linked_adapter() {
    if !program_on_path("go") {
        eprintln!("skipping: no `go` toolchain on PATH");
        return;
    }
    let sample = repo("cross-ecosystem/umbrella");
    toven_ok(&sample, &["build"])
        .expect_stderr_contains("ok rust")
        .expect_stderr_contains("ok go:services-api#build")
        .expect_stderr_contains("ok command:tools#build")
        .expect_stderr_contains("ran:  3");
}
