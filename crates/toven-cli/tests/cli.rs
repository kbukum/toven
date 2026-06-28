//! End-to-end dispatch tests for the public [`toven_cli::run_from`] entry.
//!
//! These exercise the argv surface that resolves before any project load — clap
//! help/usage outcomes and the flag-applicability gate — so they are fully
//! deterministic and need no fixture repository. Project-execution paths (PLAN /
//! APPLY against a real workspace) are covered by the managed `make smoke`
//! target, and the projection/parse/collision logic by the crate's unit tests.

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
