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
fn affected_reports_full_activation_for_an_unattributable_root_change() {
    // A root `toven.toml` edit cannot be attributed to any module, so the whole
    // set activates (fail-closed) — the diagnostic names the offending path on
    // stdout so the full run is never silent.
    let sample = repo("rust/multi-module");
    let scenario = toven_testkit::git::GitScenario::open(sample.root()).expect("open git tree");
    scenario
        .commit_file(
            "toven.toml",
            &format!(
                "{}\n# unattributable root edit\n",
                std::fs::read_to_string(sample.root().join("toven.toml")).expect("read toven.toml")
            ),
            "touch root config",
        )
        .expect("commit a root toven.toml change");

    let out = toven_rs_ok(&sample, &["affected", "build", "--base", "HEAD~1"]);
    out.expect_stdout_contains("full activation: toven.toml (affects all modules)")
        .expect_stdout_contains("rust:app")
        .expect_stdout_contains("rust:corelib")
        .expect_stdout_contains("rust:util");
}

#[test]
fn affected_reports_no_full_activation_for_a_precise_change() {
    // A change confined to one crate is precisely attributed, so no
    // full-activation diagnostic is emitted.
    let sample = repo("rust/multi-module");
    let scenario = toven_testkit::git::GitScenario::open(sample.root()).expect("open git tree");
    scenario
        .commit_file(
            "crates/corelib/src/lib.rs",
            "pub fn touched() {}\n",
            "touch corelib",
        )
        .expect("commit a corelib change");

    let out = toven_rs_ok(&sample, &["affected", "build", "--base", "HEAD~1"]);
    out.expect_stdout_contains("rust:corelib");
    assert!(
        !out.stdout.contains("full activation"),
        "a precisely attributed change must not print the diagnostic: {}",
        out.stdout
    );
}

#[test]
fn explain_module_projects_the_real_batched_unit() {
    // `--module` focuses the projection without shrinking the unit: `corelib`
    // batches with its whole cargo workspace (`util`, `corelib`, `app`), so the
    // rendered unit is that real batched argv — never a synthetic single-`-p` cut
    // — with the focused member marked and its co-batched siblings shown in full.
    let sample = repo("rust/multi-module");
    toven_rs(&sample, &["explain", "build", "--module", "rust:corelib"])
        .expect_success()
        .expect_stdout_contains("modules:  rust:util, rust:corelib, rust:app")
        .expect_stdout_contains("target:  rust:corelib")
        .expect_stdout_contains(
            "argv:  [\"cargo\", \"build\", \"--manifest-path\", \
             \"crates/util/Cargo.toml\", \"-p\", \"util\", \"-p\", \"corelib\", \"-p\", \"app\"]",
        );
}

#[test]
fn explain_module_narrows_to_units_containing_the_target() {
    // Two independent cargo workspaces (`services/a`, `services/b`) each batch
    // into their own unit. Focusing on `a-core` shows only the `a` unit; the
    // unrelated `b` unit is filtered out of the projection.
    let sample = repo("rust/multi-workspace");
    let out = toven_rs(&sample, &["explain", "build", "--module", "rust:a-core"]);
    out.expect_success()
        .expect_stdout_contains("modules:  rust:a-core, rust:a-app")
        .expect_stdout_contains("target:  rust:a-core");
    assert!(
        !out.stdout.contains("b-core") && !out.stdout.contains("b-app"),
        "the `b` workspace unit must be filtered out: {}",
        out.stdout
    );
}

#[test]
fn explain_without_a_module_shows_every_unit_without_a_target_field() {
    // The unfocused projection is unchanged: every planned unit is shown and no
    // `target` marker is rendered.
    let sample = repo("rust/multi-workspace");
    let out = toven_rs(&sample, &["explain", "build"]);
    out.expect_success()
        .expect_stdout_contains("rust:a-app")
        .expect_stdout_contains("rust:b-app");
    assert!(
        !out.stdout.contains("target:"),
        "unfocused explain must not render a target marker: {}",
        out.stdout
    );
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

#[test]
fn modules_human_table_labels_each_module_with_its_workspace() {
    let sample = repo("rust/multi-workspace");
    toven_rs_ok(&sample, &["modules"])
        .expect_stdout_contains("Workspace")
        .expect_stdout_contains("rust:services/a")
        .expect_stdout_contains("rust:services/b");
}

#[test]
fn modules_jsonl_emits_one_json_object_per_module_with_a_workspace_field() {
    let sample = repo("rust/multi-workspace");
    let stdout = toven_rs_ok(&sample, &["modules", "--output", "jsonl"]).stdout;

    let rows: Vec<serde_json::Value> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("each line is valid JSON"))
        .collect();

    assert_eq!(rows.len(), 4, "one object per discovered module");
    assert_eq!(rows[0]["module"], "rust:a-app");
    assert_eq!(rows[0]["workspace"], "rust:services/a");
    assert_eq!(rows[3]["module"], "rust:b-core");
    assert_eq!(rows[3]["workspace"], "rust:services/b");
}
