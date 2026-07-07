//! End-to-end dispatch tests for the public [`toven_cli::run_from`] entry.
//!
//! These exercise the argv surface that resolves before any project load — clap
//! help/usage outcomes and the flag-applicability gate — so they are fully
//! deterministic and need no fixture repository. Project-execution paths (PLAN /
//! APPLY against a real workspace) are covered by the in-tree app smokes
//! (`apps/toven/tests/smoke.rs`, `apps/toven-rs/tests/smoke.rs`), and the
//! projection/parse/collision logic by the crate's unit tests.

use rskit_cli::ExitCode;
use toven_ports::Provider;

/// No providers are needed: every case errors or prints before a project loads.
fn run(args: &[&str]) -> ExitCode {
    let providers: Vec<&dyn Provider> = Vec::new();
    toven_cli::run_from(
        &providers,
        std::iter::once("toven").chain(args.iter().copied()),
    )
}

#[test]
fn help_flag_succeeds() {
    assert_eq!(run(&["--help"]), ExitCode::Success);
}

#[test]
fn bare_invocation_shows_help_and_succeeds() {
    // `subcommand_required` + `arg_required_else_help` render help, which clap
    // reports as a (successful) help display rather than a usage error.
    assert_eq!(run(&[]), ExitCode::Success);
}

#[test]
fn unknown_reserved_flag_is_a_usage_error() {
    assert_eq!(
        run(&["--definitely-not-a-flag", "plan", "test"]),
        ExitCode::Usage
    );
}

#[test]
fn release_only_flag_on_a_task_is_gated_to_usage() {
    // `--allow-dirty` only applies to `toven release`; using it on `plan` is a
    // typed InvalidInput error, which maps to the usage exit code.
    assert_eq!(run(&["--allow-dirty", "plan", "test"]), ExitCode::Usage);
}

#[test]
fn graph_format_flag_on_a_non_graph_verb_is_gated_to_usage() {
    assert_eq!(run(&["--format", "dot", "plan", "test"]), ExitCode::Usage);
}

#[test]
fn init_only_flags_on_a_non_init_verb_are_gated_to_usage() {
    // `--print`/`--non-interactive` only apply to `toven init`; using them on
    // another verb is a typed InvalidInput error, mapped to the usage exit code.
    assert_eq!(run(&["--print", "plan", "test"]), ExitCode::Usage);
    assert_eq!(run(&["--non-interactive", "plan", "test"]), ExitCode::Usage);
}

#[test]
fn execution_flags_on_a_cache_verb_are_gated_to_usage() {
    assert_eq!(run(&["--dry-run", "cache", "path"]), ExitCode::Usage);
}

#[test]
fn execution_flags_on_introspection_verbs_are_gated_to_usage() {
    assert_eq!(run(&["--output", "jsonl", "modules"]), ExitCode::Usage);
    assert_eq!(run(&["--fail-fast", "affected", "test"]), ExitCode::Usage);
}

#[test]
fn auto_install_on_no_op_provisioning_verbs_is_gated_to_usage() {
    // `--auto-install` only acts on `driver list` / `federation sync`; on the
    // verbs where it would be a silent no-op (an explicit install, or a
    // read-only status) it is rejected rather than advertised.
    assert_eq!(
        run(&["--auto-install", "driver", "install", "go"]),
        ExitCode::Usage
    );
    assert_eq!(
        run(&["--auto-install", "federation", "status"]),
        ExitCode::Usage
    );
}

#[test]
fn explicit_selection_flags_on_a_non_selection_verb_are_gated_to_usage() {
    // `--module`/`--workspace`/`--with-dependents` only shape selection, which
    // the execution/`affected` verbs perform; on `modules` they are rejected.
    assert_eq!(run(&["--module", "rust:core", "modules"]), ExitCode::Usage);
    assert_eq!(run(&["--workspace", "rust", "modules"]), ExitCode::Usage);
    assert_eq!(run(&["--with-dependents", "modules"]), ExitCode::Usage);
}

#[test]
fn watch_with_a_plan_only_cut_on_a_bare_task_is_gated_to_usage() {
    // Trailing task flags land in `Command::External`, so the pre-token gate
    // never sees them; the merged watch-combination gate must still reject a
    // watch loop paired with a PLAN-only cut, before any project load.
    assert_eq!(run(&["test", "--watch", "--dry-run"]), ExitCode::Usage);
    assert_eq!(run(&["test", "--watch", "--explain"]), ExitCode::Usage);
    // Mixed arrival: a global PLAN-only cut with a per-task `--watch` token.
    assert_eq!(run(&["--dry-run", "test", "--watch"]), ExitCode::Usage);
}

#[test]
fn completions_prints_a_script_and_succeeds() {
    // `completions` is a pure projection: it needs no project load, so it is
    // deterministic here and exits success for every supported shell.
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        assert_eq!(run(&["completions", shell]), ExitCode::Success, "{shell}");
    }
}

#[test]
fn completions_with_an_unknown_shell_is_a_usage_error() {
    assert_eq!(run(&["completions", "commodore-64"]), ExitCode::Usage);
}

#[test]
fn color_flag_rejects_an_unknown_policy_as_a_usage_error() {
    // The `--color` value set is closed; an unknown policy is a clap parse
    // failure (usage), never a silent fallback to auto.
    assert_eq!(run(&["--color", "sometimes", "modules"]), ExitCode::Usage);
}

#[test]
fn watch_debounce_without_watch_on_a_bare_task_is_gated_to_usage() {
    assert_eq!(
        run(&["test", "--watch-debounce-ms", "500"]),
        ExitCode::Usage
    );
}
