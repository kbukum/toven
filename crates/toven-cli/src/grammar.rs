//! Argv-first dispatch grammar: the reserved-word set and the bare-task tail
//! parser.
//!
//! `toven <token>` is argv-first: a token in the [reserved set](RESERVED) routes
//! to a built-in (modeled by [`Command`](crate::flags::Command)); anything else
//! is a task name. Because clap captures a bare task name as an
//! [`External`](crate::flags::Command::External) subcommand — collecting the
//! token plus every following arg verbatim — the execution flags and `--`
//! passthrough that trail a task are re-parsed here, keeping user argv sacred:
//! only recognized execution flags are consumed and everything after `--` is
//! carried through untouched.

use std::path::PathBuf;

use rskit_errors::{AppError, AppResult};

use crate::flags::OutputKind;

/// The reserved built-in words. A bare top-level token equal to one of these
/// dispatches the built-in; any other token is an argv-first task name.
pub const RESERVED: &[&str] = &[
    "run",
    "plan",
    "release",
    "explain",
    "generate",
    "affected",
    "modules",
    "list",
    "ls",
    "graph",
    "deps",
    "driver",
    "federation",
    "cache",
    "help",
];

/// Whether `token` is a reserved built-in word.
#[must_use]
pub fn is_reserved(token: &str) -> bool {
    RESERVED.contains(&token)
}

/// Execution flags that may trail a bare task name.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct TaskFlags {
    /// `--config <path>` override.
    pub config: Option<PathBuf>,
    /// `--output <format>` override.
    pub output: Option<OutputKind>,
    /// `--dry-run`.
    pub dry_run: bool,
    /// `--explain`.
    pub explain: bool,
    /// `--fail-fast`.
    pub fail_fast: bool,
    /// `--base <ref>`: override the changed-selection baseline reference.
    pub base: Option<String>,
    /// `--merge-base`: diff against `merge-base(reference, HEAD)`.
    pub merge_base: bool,
    /// `-v`/`--verbose` repeat count.
    pub verbose: u8,
    /// `-q`/`--quiet` repeat count.
    pub quiet: u8,
}

/// A resolved bare-task invocation: the task name, its execution flags, and the
/// verbatim passthrough that followed `--`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskInvocation {
    /// The argv-first task name.
    pub task: String,
    /// Recognized execution flags that trailed the task name.
    pub flags: TaskFlags,
    /// Passthrough args after `--`, carried verbatim and never rewritten.
    pub passthrough: Vec<String>,
}

/// Parse the `External` token vector for a bare task: `<task> [exec-flags] [-- passthrough]`.
///
/// Only execution flags are recognized before `--`; a verb-specific flag
/// (`--allow-dirty`, `--force`, …) or any other unknown flag is a typed usage
/// error pointing at the verb it belongs to. Everything after `--` is passthrough
/// and is never interpreted.
///
/// # Errors
/// Returns a usage error for an empty token vector, a flag that requires a value
/// but is missing one, an unknown/misplaced flag, or a verb-specific flag used on
/// a task.
pub fn parse_task(tokens: &[String]) -> AppResult<TaskInvocation> {
    let mut iter = tokens.iter();
    let task = iter
        .next()
        .filter(|token| !token.is_empty())
        .ok_or_else(|| AppError::invalid_input("command", "expected a task name"))?
        .clone();

    let mut flags = TaskFlags::default();
    let mut passthrough = Vec::new();
    while let Some(token) = iter.next() {
        if token == "--" {
            passthrough.extend(iter.cloned());
            break;
        }
        match token.as_str() {
            "--dry-run" => flags.dry_run = true,
            "--explain" => flags.explain = true,
            "--fail-fast" => flags.fail_fast = true,
            "--merge-base" => flags.merge_base = true,
            "--base" => flags.base = Some(value_for("--base", &mut iter)?),
            "-v" | "--verbose" => flags.verbose = flags.verbose.saturating_add(1),
            "-q" | "--quiet" => flags.quiet = flags.quiet.saturating_add(1),
            "--config" => flags.config = Some(PathBuf::from(value_for("--config", &mut iter)?)),
            "--output" => flags.output = Some(parse_output(&value_for("--output", &mut iter)?)?),
            other => return Err(reject_flag(other, &task)),
        }
    }

    Ok(TaskInvocation {
        task,
        flags,
        passthrough,
    })
}

/// Consume the next token as the value for a value-taking flag.
fn value_for<'a>(flag: &str, iter: &mut impl Iterator<Item = &'a String>) -> AppResult<String> {
    let Some(value) = iter.next() else {
        return Err(AppError::invalid_input(
            flag,
            format!("`{flag}` requires a value"),
        ));
    };
    if value == "--" {
        return Err(AppError::invalid_input(
            flag,
            format!("`{flag}` requires a value before passthrough separator `--`"),
        ));
    }
    Ok(value.clone())
}

fn parse_output(value: &str) -> AppResult<OutputKind> {
    match value {
        "human" => Ok(OutputKind::Human),
        "jsonl" => Ok(OutputKind::Jsonl),
        other => Err(AppError::invalid_input(
            "--output",
            format!("unknown output format `{other}` (expected `human` or `jsonl`)"),
        )),
    }
}

/// Build the typed error for a flag that is not a valid task execution flag.
fn reject_flag(flag: &str, task: &str) -> AppError {
    let owner = match flag {
        "--allow-dirty" | "--no-push" => Some("toven release"),
        "--force" | "--root" => Some("toven generate"),
        "--format" => Some("toven graph"),
        "--auto-install" => Some("toven driver / toven federation"),
        _ => None,
    };
    owner.map_or_else(
        || {
            AppError::invalid_input(
                "flags",
                format!(
                    "unrecognized flag `{flag}` for task `{task}` (use `--` to pass args through to the task)"
                ),
            )
        },
        |owner| {
            AppError::invalid_input(
                "flags",
                format!("`{flag}` only applies to `{owner}`, not task `{task}`"),
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::{is_reserved, parse_task};
    use crate::flags::OutputKind;

    fn tokens(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_string()).collect()
    }

    #[test]
    fn reserved_words_are_recognized() {
        assert!(is_reserved("plan"));
        assert!(is_reserved("graph"));
        assert!(!is_reserved("test"));
        assert!(!is_reserved("build"));
    }

    #[test]
    fn parses_bare_task_with_no_flags() {
        let invocation = parse_task(&tokens(&["test"])).expect("parses");
        assert_eq!(invocation.task, "test");
        assert!(invocation.passthrough.is_empty());
        assert!(!invocation.flags.dry_run);
    }

    #[test]
    fn parses_execution_flags_and_passthrough_verbatim() {
        let invocation = parse_task(&tokens(&[
            "test",
            "--dry-run",
            "--output",
            "jsonl",
            "--",
            "--nocapture",
            "--flag",
        ]))
        .expect("parses");
        assert_eq!(invocation.task, "test");
        assert!(invocation.flags.dry_run);
        assert_eq!(invocation.flags.output, Some(OutputKind::Jsonl));
        assert_eq!(invocation.passthrough, vec!["--nocapture", "--flag"]);
    }

    #[test]
    fn passthrough_is_untouched_even_when_it_looks_like_a_reserved_flag() {
        let invocation =
            parse_task(&tokens(&["test", "--", "--allow-dirty", "--force"])).expect("parses");
        assert_eq!(invocation.passthrough, vec!["--allow-dirty", "--force"]);
    }

    #[test]
    fn verb_specific_flag_on_a_task_is_rejected_with_owner() {
        let error = parse_task(&tokens(&["test", "--allow-dirty"])).expect_err("rejected");
        assert!(error.to_string().contains("toven release"));
    }

    #[test]
    fn unknown_flag_on_a_task_is_rejected() {
        assert!(parse_task(&tokens(&["test", "--bogus"])).is_err());
    }

    #[test]
    fn parses_baseline_flags_on_a_bare_task() {
        let invocation = parse_task(&tokens(&["test", "--base", "origin/main", "--merge-base"]))
            .expect("parses");
        assert_eq!(invocation.task, "test");
        assert_eq!(invocation.flags.base.as_deref(), Some("origin/main"));
        assert!(invocation.flags.merge_base);
    }

    #[test]
    fn missing_flag_value_is_rejected() {
        assert!(parse_task(&tokens(&["test", "--output"])).is_err());
    }

    #[test]
    fn passthrough_separator_is_not_a_flag_value() {
        assert!(parse_task(&tokens(&["test", "--config", "--", "--flag"])).is_err());
        assert!(parse_task(&tokens(&["test", "--output", "--", "--flag"])).is_err());
    }

    #[test]
    fn empty_token_vector_is_rejected() {
        assert!(parse_task(&[]).is_err());
    }
}
