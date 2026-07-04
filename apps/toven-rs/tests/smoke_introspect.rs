//! Standalone `toven-rs` introspection smokes against real Rust fixtures:
//! `modules`/`list`/`ls`, `graph` (text + dot), `affected`, and `explain`.
//! Read-only, deterministic, toolchain-independent. Projections render to
//! **stdout**.

mod common;

use common::{repo, toven_rs, toven_rs_ok};

#[test]
fn modules_lists_the_single_crate_baseline() {
    let sample = repo("rust/single");
    toven_rs_ok(&sample, &["modules"]).expect_stdout_contains("rust:app");
}

#[test]
fn modules_aliases_project_the_same_set() {
    let sample = repo("rust/multi-module");
    let canonical = toven_rs_ok(&sample, &["modules"]).stdout;
    for alias in ["list", "ls"] {
        assert_eq!(
            toven_rs_ok(&sample, &[alias]).stdout,
            canonical,
            "alias {alias}"
        );
    }
}

#[test]
fn graph_text_and_dot_render_the_dependency_chain() {
    let sample = repo("rust/multi-module");
    toven_rs_ok(&sample, &["graph"])
        .expect_stdout_contains("rust:app")
        .expect_stdout_contains("  -> rust:corelib")
        .expect_stdout_contains("  -> rust:util");

    let dot = toven_rs_ok(&sample, &["graph", "--format", "dot"]);
    assert!(dot.stdout.trim_start().starts_with("digraph toven {"));
    dot.expect_stdout_contains("\"rust:app\" -> \"rust:corelib\";")
        .expect_stdout_contains("\"rust:corelib\" -> \"rust:util\";");
}

#[test]
fn affected_without_a_diff_reports_the_full_set() {
    let sample = repo("rust/multi-module");
    toven_rs_ok(&sample, &["affected", "build"])
        .expect_stdout_contains("rust:app")
        .expect_stdout_contains("rust:corelib")
        .expect_stdout_contains("rust:util");
}

#[test]
fn explain_renders_the_planned_unit_for_a_module_and_task() {
    // Regression lock for the flags.rs id-collision fix: the positional
    // `<module>` must not be swallowed by the global `--module` flag.
    let sample = repo("rust/multi-module");
    toven_rs(&sample, &["explain", "rust:corelib", "build"])
        .expect_success()
        .expect_stdout_contains("rust:corelib")
        .expect_stdout_contains("task:")
        .expect_stdout_contains("argv:");
}

#[test]
fn multiple_workspaces_are_discovered_under_one_project() {
    let sample = repo("rust/multi-workspace");
    toven_rs_ok(&sample, &["modules"])
        .expect_stdout_contains("rust:a-app")
        .expect_stdout_contains("rust:a-core")
        .expect_stdout_contains("rust:b-app")
        .expect_stdout_contains("rust:b-core");
}
