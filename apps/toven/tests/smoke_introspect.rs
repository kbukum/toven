//! Umbrella `toven` introspection smokes: `modules`/`list`/`ls`, `graph`
//! (text + dot), `affected`, and `explain` — all read-only, deterministic, and
//! toolchain-independent. Introspection projections render to **stdout**.

mod common;

use common::{repo, toven, toven_ok};

#[test]
fn modules_lists_every_ecosystem_of_a_multi_eco_project() {
    let sample = repo("cross-ecosystem/umbrella");
    let out = toven_ok(&sample, &["modules"]);
    for module in ["rust:app", "go:api", "command:tools"] {
        assert!(
            out.stdout.contains(module),
            "`modules` should list {module}, got:\n{}",
            out.stdout
        );
    }
}

#[test]
fn modules_aliases_project_the_same_set() {
    let sample = repo("cross-ecosystem/umbrella");
    let canonical = toven_ok(&sample, &["modules"]).stdout;
    for alias in ["list", "ls"] {
        let aliased = toven_ok(&sample, &[alias]).stdout;
        assert_eq!(aliased, canonical, "alias `{alias}` should match `modules`");
    }
}

#[test]
fn graph_text_renders_dependency_edges() {
    let sample = repo("rust/multi-module");
    toven_ok(&sample, &["graph"])
        .expect_stdout_contains("rust:app")
        .expect_stdout_contains("  -> rust:corelib")
        .expect_stdout_contains("  -> rust:util");
}

#[test]
fn graph_dot_emits_a_digraph_with_quoted_edges() {
    let sample = repo("rust/multi-module");
    let out = toven_ok(&sample, &["graph", "--format", "dot"]);
    assert!(
        out.stdout.trim_start().starts_with("digraph toven {"),
        "dot output should open a digraph, got:\n{}",
        out.stdout
    );
    out.expect_stdout_contains("\"rust:app\" -> \"rust:corelib\";")
        .expect_stdout_contains("\"rust:corelib\" -> \"rust:util\";");
}

#[test]
fn affected_without_a_diff_reports_the_full_set() {
    let sample = repo("rust/multi-module");
    toven_ok(&sample, &["affected", "build"])
        .expect_stdout_contains("rust:app")
        .expect_stdout_contains("rust:corelib")
        .expect_stdout_contains("rust:util");
}

#[test]
fn affected_narrows_to_a_touched_module_and_its_dependents() {
    let sample = repo("go/multi-module");
    let scenario = toven_testkit::git::GitScenario::open(sample.root()).expect("open git tree");
    scenario
        .commit_file(
            "core/core.go",
            "package core\n\nfunc Greeting() string { return \"changed\" }\n",
            "touch core",
        )
        .expect("commit a one-file change to core");

    // With `core` changed, the affected set is `core` plus its dependent `app`.
    let out = toven_ok(&sample, &["affected", "build", "--base", "HEAD~1"]);
    out.expect_stdout_contains("go:core")
        .expect_stdout_contains("go:app");
}

#[test]
fn explain_renders_the_planned_unit_for_a_module_and_task() {
    // Regression lock for the flags.rs id-collision fix: the positional
    // `<module>` must not be swallowed by the global `--module` selection flag.
    let sample = repo("rust/multi-module");
    let out = toven(&sample, &["explain", "rust:corelib", "build"]);
    out.expect_success()
        .expect_stdout_contains("module:")
        .expect_stdout_contains("rust:corelib")
        .expect_stdout_contains("task:")
        .expect_stdout_contains("argv:");
}

#[test]
fn tasks_lists_the_canonical_runnable_task_names_per_ecosystem() {
    // The catalog projects the *canonical* task name (`format`, never the `fmt`
    // shorthand), so users address tasks by the name Toven actually resolves.
    let sample = repo("rust/multi-module");
    let out = toven_ok(&sample, &["tasks"]);
    out.expect_stdout_contains("rust tasks")
        .expect_stdout_contains("format")
        .expect_stdout_contains("build")
        .expect_stdout_contains("test");
    assert!(
        !out.stdout.contains(" fmt "),
        "the catalog must project the canonical `format`, not `fmt`, got:\n{}",
        out.stdout
    );
}

#[test]
fn tasks_detail_shows_a_task_argv_and_inputs() {
    let sample = repo("rust/multi-module");
    toven_ok(&sample, &["tasks", "format"])
        .expect_stdout_contains("task:")
        .expect_stdout_contains("format")
        .expect_stdout_contains("argv:")
        .expect_stdout_contains("cargo fmt");
}

#[test]
fn tasks_jsonl_emits_a_stable_record_per_task() {
    // The `--output jsonl` projection is a machine surface: one JSON object per
    // task on stdout, carrying the stable field set scripts depend on. Lock the
    // schema (keys + canonical values) so a regression is caught here.
    let sample = repo("rust/multi-module");
    let out = toven_ok(&sample, &["tasks", "--output", "jsonl"]);
    let lines: Vec<&str> = out.stdout.lines().filter(|l| !l.is_empty()).collect();
    assert!(
        !lines.is_empty(),
        "jsonl output should emit at least one record, got:\n{}",
        out.stdout
    );
    for line in &lines {
        let record: serde_json::Value =
            serde_json::from_str(line).expect("each jsonl line must be a JSON object");
        for key in [
            "ecosystem",
            "task",
            "kind",
            "origin",
            "fan_out",
            "persistent",
            "argv",
            "shared_inputs",
        ] {
            assert!(
                record.get(key).is_some(),
                "jsonl record is missing `{key}`, got:\n{line}"
            );
        }
        assert_eq!(record["ecosystem"], "rust");
    }
    // The canonical `format` name (never the `fmt` shorthand) survives into the
    // machine schema exactly as it does in the human table.
    assert!(
        lines
            .iter()
            .any(|l| l.contains("\"task\":\"format\"") && l.contains("\"kind\":\"format\"")),
        "jsonl must project the canonical `format` task, got:\n{}",
        out.stdout
    );
}

#[test]
fn unknown_task_that_typos_a_reserved_verb_advises_the_builtin() {
    // argv stays sacred: `modual` first fails to resolve as a task, and only
    // then does the advisory point at the near-miss built-in `modules`. Lock the
    // typo-hint wiring so the guidance keeps reaching the error surface (stderr).
    let sample = repo("rust/multi-module");
    let out = toven(&sample, &["modual"]);
    out.expect_code(2)
        .expect_stderr_contains("has no 'modual' task")
        .expect_stderr_contains("If you meant the built-in, run 'toven modules'.");
}

#[test]
fn completions_emit_a_shell_script_for_each_supported_shell() {
    // `completions` is a pure projection (no project load); it prints the script
    // to stdout and exits success for every supported shell.
    let sample = repo("rust/multi-module");
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        let out = toven(&sample, &["completions", shell]);
        out.expect_success();
        assert!(
            out.stdout.contains("toven"),
            "the {shell} completion script should mention the binary, got:\n{}",
            out.stdout
        );
    }
}
