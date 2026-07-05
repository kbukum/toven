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
use std::time::Duration;

use rskit_errors::{AppError, AppResult};

use crate::flags::{OutputKind, parse_duration_arg};

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
    "tasks",
    "completions",
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

/// The maximum edit distance for a token to be treated as a typo of a reserved
/// built-in verb.
///
/// Looser than the default suggestion distance because the reserved set is small
/// and closed, and the hint is only ever offered *after* the token already
/// failed to resolve as a task — so a slightly wider net (catching `modual` →
/// `modules`) carries little risk of a misleading suggestion.
const RESERVED_SUGGESTION_DISTANCE: usize = 3;

/// The reserved built-in nearest to `token` within
/// [`RESERVED_SUGGESTION_DISTANCE`], or `None` when it is not a plausible typo of
/// any built-in.
///
/// Advisory only: the argv-first dispatch never uses this to redirect input — it
/// feeds the "did you mean the built-in?" hint after a token has already failed
/// to resolve as a task. Exact reserved words are handled by dispatch and so are
/// excluded here.
#[must_use]
pub fn nearest_reserved(token: &str) -> Option<&'static str> {
    if is_reserved(token) {
        return None;
    }
    rskit_util::strings::nearest_within(
        token,
        RESERVED.iter().copied(),
        RESERVED_SUGGESTION_DISTANCE,
    )
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
    /// `--no-cache`: bypass the task cache (re-run every unit; no read/write).
    pub no_cache: bool,
    /// `--refresh`: ignore cached results and re-run, but still write results.
    pub refresh: bool,
    /// `--timeout <dur>`: per-unit execution bound (e.g. `30s`, `5m`).
    pub timeout: Option<Duration>,
    /// `--base <ref>`: override the changed-selection baseline reference.
    pub base: Option<String>,
    /// `--merge-base`: diff against `merge-base(reference, HEAD)`.
    pub merge_base: bool,
    /// `--module <ref>`: explicit graph target (`ecosystem:name`), repeatable.
    pub modules: Vec<String>,
    /// `--workspace <id>`: explicit graph target, repeatable.
    pub workspaces: Vec<String>,
    /// `--with-dependents`: also activate the reverse-dependents closure.
    pub with_dependents: bool,
    /// `--watch`: rerun the affected subgraph on filesystem changes.
    pub watch: bool,
    /// `--watch-debounce-ms <n>`: debounce window for coalescing changes.
    pub watch_debounce_ms: Option<u64>,
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
    /// Passthrough args carried verbatim and never rewritten. Begins at either an
    /// explicit `--` or the first token that is not a recognized Toven flag.
    pub passthrough: Vec<String>,
}

/// Parse the `External` token vector for a bare task: `<task> [toven-flags...] [args...]`.
///
/// Toven's own execution/selection flags are consumed only as a *contiguous
/// prefix* immediately after the task name. The first token that is not a
/// recognized Toven flag — a positional argument or an unknown flag — begins the
/// task's own argument vector: it and every token after it are carried through
/// verbatim, never interpreted. An explicit `--` forces the boundary early. This
/// keeps `toven test <the task's own args...>` friction-free: users pass their
/// command's parameters without escaping, and only the leading Toven flags they
/// deliberately place before them are absorbed.
///
/// # Errors
/// Returns a usage error for an empty token vector, or a Toven value-flag in the
/// prefix that is missing its value.
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
            "--no-cache" => flags.no_cache = true,
            "--refresh" => flags.refresh = true,
            "--timeout" => {
                flags.timeout = Some(parse_timeout(&value_for("--timeout", &mut iter)?)?);
            }
            "--merge-base" => flags.merge_base = true,
            "--with-dependents" => flags.with_dependents = true,
            "--watch" => flags.watch = true,
            "--watch-debounce-ms" => {
                flags.watch_debounce_ms = Some(parse_debounce(&value_for(
                    "--watch-debounce-ms",
                    &mut iter,
                )?)?);
            }
            "--base" => flags.base = Some(value_for("--base", &mut iter)?),
            "--module" => flags.modules.push(value_for("--module", &mut iter)?),
            "--workspace" => flags.workspaces.push(value_for("--workspace", &mut iter)?),
            "-v" | "--verbose" => flags.verbose = flags.verbose.saturating_add(1),
            "-q" | "--quiet" => flags.quiet = flags.quiet.saturating_add(1),
            "--config" => flags.config = Some(PathBuf::from(value_for("--config", &mut iter)?)),
            "--output" => flags.output = Some(parse_output(&value_for("--output", &mut iter)?)?),
            // First non-Toven token ends the prefix: it and the rest are the
            // task's own argv, spliced verbatim and never rewritten.
            _ => {
                passthrough.push(token.clone());
                passthrough.extend(iter.cloned());
                break;
            }
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

/// Parse the `--watch-debounce-ms` value as a non-negative millisecond count.
fn parse_debounce(value: &str) -> AppResult<u64> {
    value.parse::<u64>().map_err(|error| {
        AppError::invalid_input(
            "--watch-debounce-ms",
            format!(
                "`--watch-debounce-ms` requires a non-negative integer (got `{value}`): {error}"
            ),
        )
    })
}

/// Parse the `--timeout` value as a per-unit execution duration.
///
/// Delegates to [`parse_duration_arg`](crate::flags::parse_duration_arg) so the
/// trailing-token path and the clap global agree on accepted syntax and errors,
/// lifting its `String` message into a typed usage error.
fn parse_timeout(value: &str) -> AppResult<Duration> {
    parse_duration_arg(value).map_err(|message| AppError::invalid_input("--timeout", message))
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
        assert!(is_reserved("tasks"));
        assert!(is_reserved("completions"));
        assert!(!is_reserved("test"));
        assert!(!is_reserved("build"));
    }

    #[test]
    fn nearest_reserved_suggests_a_typo_but_not_an_exact_verb() {
        use super::nearest_reserved;
        assert_eq!(nearest_reserved("modual"), Some("modules"));
        assert_eq!(nearest_reserved("graf"), Some("graph"));
        // An exact reserved word is dispatched, never suggested.
        assert_eq!(nearest_reserved("modules"), None);
        // A far-off token (a genuine task name) yields no built-in hint.
        assert_eq!(nearest_reserved("integration"), None);
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
    fn a_previously_verb_specific_flag_now_passes_through_to_the_task() {
        // Friction-free passthrough: the first non-Toven token ends the prefix,
        // so a flag Toven does not own becomes the task's own argument.
        let invocation = parse_task(&tokens(&["test", "--allow-dirty"])).expect("parses");
        assert_eq!(invocation.task, "test");
        assert_eq!(invocation.passthrough, vec!["--allow-dirty"]);
    }

    #[test]
    fn an_unknown_flag_becomes_task_passthrough_with_the_rest() {
        let invocation =
            parse_task(&tokens(&["test", "--bogus", "value", "--more"])).expect("parses");
        assert_eq!(invocation.task, "test");
        assert_eq!(invocation.passthrough, vec!["--bogus", "value", "--more"]);
    }

    #[test]
    fn toven_flags_before_the_first_task_argument_are_consumed() {
        let invocation = parse_task(&tokens(&["test", "--dry-run", "--nocapture", "--dry-run"]))
            .expect("parses");
        assert!(invocation.flags.dry_run);
        // The Toven-looking `--dry-run` after the first task arg is not re-parsed.
        assert_eq!(invocation.passthrough, vec!["--nocapture", "--dry-run"]);
    }

    #[test]
    fn a_leading_positional_ends_the_prefix() {
        let invocation =
            parse_task(&tokens(&["test", "integration", "--dry-run"])).expect("parses");
        assert!(!invocation.flags.dry_run);
        assert_eq!(invocation.passthrough, vec!["integration", "--dry-run"]);
    }

    #[test]
    fn parses_explicit_selection_flags_on_a_bare_task() {
        let invocation = parse_task(&tokens(&[
            "test",
            "--module",
            "rust:core",
            "--workspace",
            "rust",
            "--with-dependents",
        ]))
        .expect("parses");
        assert_eq!(invocation.flags.modules, vec!["rust:core"]);
        assert_eq!(invocation.flags.workspaces, vec!["rust"]);
        assert!(invocation.flags.with_dependents);
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
    fn parses_no_cache_on_a_bare_task() {
        let invocation = parse_task(&tokens(&["test", "--no-cache"])).expect("parses");
        assert_eq!(invocation.task, "test");
        assert!(invocation.flags.no_cache);
    }

    #[test]
    fn parses_refresh_on_a_bare_task() {
        let invocation = parse_task(&tokens(&["test", "--refresh"])).expect("parses");
        assert_eq!(invocation.task, "test");
        assert!(invocation.flags.refresh);
        assert!(!invocation.flags.no_cache);
    }

    #[test]
    fn parses_timeout_duration_on_a_bare_task() {
        let invocation = parse_task(&tokens(&["test", "--timeout", "5s"])).expect("parses");
        assert_eq!(invocation.task, "test");
        assert_eq!(
            invocation.flags.timeout,
            Some(std::time::Duration::from_secs(5))
        );
    }

    #[test]
    fn rejects_a_malformed_or_zero_timeout() {
        assert!(parse_task(&tokens(&["test", "--timeout", "soon"])).is_err());
        assert!(parse_task(&tokens(&["test", "--timeout", "0s"])).is_err());
    }

    #[test]
    fn a_missing_timeout_value_is_rejected() {
        assert!(parse_task(&tokens(&["test", "--timeout"])).is_err());
    }

    #[test]
    fn parses_watch_flags_on_a_bare_task() {
        let invocation = parse_task(&tokens(&["test", "--watch", "--watch-debounce-ms", "500"]))
            .expect("parses");
        assert_eq!(invocation.task, "test");
        assert!(invocation.flags.watch);
        assert_eq!(invocation.flags.watch_debounce_ms, Some(500));
    }

    #[test]
    fn rejects_a_non_integer_watch_debounce() {
        assert!(parse_task(&tokens(&["test", "--watch", "--watch-debounce-ms", "soon"])).is_err());
        assert!(parse_task(&tokens(&["test", "--watch", "--watch-debounce-ms", "-5"])).is_err());
    }

    #[test]
    fn a_missing_watch_debounce_value_is_rejected() {
        assert!(parse_task(&tokens(&["test", "--watch-debounce-ms"])).is_err());
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
