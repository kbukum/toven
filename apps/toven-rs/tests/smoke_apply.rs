//! Standalone `toven-rs` APPLY smokes: real `cargo` subprocess execution. These
//! always run (the test toolchain provides `cargo`). APPLY status + summary
//! lines render to **stderr**.

mod common;

use common::{repo, toven_rs_ok};

#[test]
fn bare_task_dispatch_applies_a_workspace() {
    let sample = repo("rust/workspace-linear");
    toven_rs_ok(&sample, &["check"])
        .expect_stderr_contains("  ok ")
        .expect_stderr_contains("ran:  1");
}

#[test]
fn plan_only_cut_then_full_apply_of_the_single_crate_baseline() {
    let sample = repo("rust/single");
    toven_rs_ok(&sample, &["plan", "build"]).expect_stderr_contains("ran:  0");
    toven_rs_ok(&sample, &["build"]).expect_stderr_contains("  ok ");
}

#[test]
fn argv_passthrough_splices_into_cargo_verbatim() {
    // `format -- --check` proves the `--` tail reaches `cargo fmt --check`
    // unchanged; the fixtures are rustfmt-clean, so it stays green.
    let sample = repo("rust/workspace-linear");
    toven_rs_ok(&sample, &["format", "--", "--check"]).expect_stderr_contains("  ok ");
}

#[test]
fn workspace_selection_limits_apply_to_one_workspace() {
    // Two independent cargo workspaces yield two units; `--workspace` targets one.
    let sample = repo("rust/multi-workspace");
    toven_rs_ok(&sample, &["--workspace", "rust:services/a", "check"])
        .expect_stderr_contains("ok rust@rust:services/a#check")
        .expect_stderr_contains("ran:  1");
}

#[test]
fn module_selection_activates_a_module_explicitly() {
    let sample = repo("rust/multi-workspace");
    toven_rs_ok(&sample, &["--module", "rust:a-app", "check"])
        .expect_stderr_contains("ok rust@rust:services/a#check")
        .expect_stderr_contains("ran:  1");
}
