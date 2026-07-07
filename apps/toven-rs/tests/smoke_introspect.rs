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
fn affected_resolves_a_bare_module_name() {
    // A bare, unambiguous name resolves to its canonical `ecosystem:module`.
    let sample = repo("rust/multi-module");
    let out = toven_rs_ok(&sample, &["affected", "build", "--module", "corelib"]);
    out.expect_stdout_contains("rust:corelib");
    assert!(!out.stdout.contains("rust:app"), "{}", out.stdout);
}

#[test]
fn affected_resolves_a_glob_selector_to_an_explicit_set() {
    let sample = repo("rust/multi-module");
    toven_rs_ok(&sample, &["affected", "build", "--module", "rust:*"])
        .expect_stdout_contains("rust:app")
        .expect_stdout_contains("rust:corelib")
        .expect_stdout_contains("rust:util");
}

#[test]
fn affected_dependencies_closure_adds_prerequisites() {
    // `rust:app -> rust:corelib -> rust:util`; `--dependencies` pulls the chain.
    let sample = repo("rust/multi-module");
    toven_rs_ok(
        &sample,
        &[
            "affected",
            "build",
            "--module",
            "rust:app",
            "--dependencies",
        ],
    )
    .expect_stdout_contains("rust:app")
    .expect_stdout_contains("rust:corelib")
    .expect_stdout_contains("rust:util");
}

#[test]
fn affected_dependents_closure_adds_reverse_dependents() {
    let sample = repo("rust/multi-module");
    toven_rs_ok(
        &sample,
        &["affected", "build", "--module", "rust:util", "--dependents"],
    )
    .expect_stdout_contains("rust:app")
    .expect_stdout_contains("rust:corelib")
    .expect_stdout_contains("rust:util");
}

#[test]
fn affected_combined_closures_union_dependencies_and_dependents() {
    // `--dependencies --dependents` unions both closures over the seed:
    // corelib pulls its dependency (util) and its dependent (app).
    let sample = repo("rust/multi-module");
    toven_rs_ok(
        &sample,
        &[
            "affected",
            "build",
            "--module",
            "rust:corelib",
            "--dependencies",
            "--dependents",
        ],
    )
    .expect_stdout_contains("rust:app")
    .expect_stdout_contains("rust:corelib")
    .expect_stdout_contains("rust:util");
}

#[test]
fn affected_rejects_a_selector_that_matches_nothing() {
    let sample = repo("rust/multi-module");
    let out = toven_rs(&sample, &["affected", "build", "--module", "rust:ghost"]);
    assert!(!out.success(), "{}", out.stdout);
    out.expect_stderr_contains("no module matches");
}

#[test]
fn explain_renders_the_planned_unit_for_a_module_and_task() {
    let sample = repo("rust/multi-module");
    toven_rs(&sample, &["explain", "build", "--module", "rust:corelib"])
        .expect_success()
        .expect_stdout_contains("rust:corelib")
        .expect_stdout_contains("task:")
        .expect_stdout_contains("argv:");
}

#[test]
fn explain_resolves_a_named_extra_task_by_its_addressable_name() {
    // Regression: a named extra (`test-integration`, kind = "test") advertised by
    // discovery must be schedulable by the exact token the user types, and must
    // not collide with the plain `test` task.
    let sample = repo("rust/single");
    // The `test-integration` token resolves the named extra's argv (`nextest`).
    toven_rs(
        &sample,
        &["explain", "test-integration", "--module", "rust:app"],
    )
    .expect_success()
    .expect_stdout_contains("nextest");
    // The plain `test` token still resolves the unnamed Test task (`cargo test`),
    // independently of the named extra.
    let plain = toven_rs(&sample, &["explain", "test", "--module", "rust:app"]);
    plain.expect_success().expect_stdout_contains("cargo");
    assert!(
        !plain.stdout.contains("nextest"),
        "the plain `test` token must not resolve the `test-integration` extra: {}",
        plain.stdout
    );
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
