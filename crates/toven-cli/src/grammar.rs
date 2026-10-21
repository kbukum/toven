//! Argv-first dispatch grammar: the reserved-word set and the bare-task flag
//! prefix parser.
//!
//! `toven <token>` is argv-first: a [reserved](is_reserved) token routes to a
//! built-in (modeled by [`Command`](crate::flags::Command)); anything else is a
//! task name. Clap captures a bare task name as an
//! [`External`](crate::flags::Command::External) subcommand with every
//! following arg verbatim, so the flags that trail a task are parsed here with
//! the same clap group the global flags use, [`ExecutionArgs`]. User argv stays
//! sacred: only a contiguous prefix of recognized flags is consumed, `--` ends
//! the prefix early, and the first other token plus everything after it pass
//! through untouched.

use std::collections::HashMap;
use std::sync::LazyLock;

use clap::{CommandFactory, FromArgMatches, Parser};
use rskit_errors::{AppError, AppResult};

use crate::flags::{Cli, ExecutionArgs};

/// The reserved words: every built-in verb name and alias from the clap
/// command tree, plus `help`. Derived, so a new verb is reserved by
/// construction.
static RESERVED: LazyLock<Vec<String>> = LazyLock::new(|| {
    let command = Cli::command();
    let mut words: Vec<String> = command
        .get_subcommands()
        .flat_map(|verb| std::iter::once(verb.get_name()).chain(verb.get_all_aliases()))
        .map(str::to_owned)
        .collect();
    words.push("help".to_owned());
    words.sort();
    words.dedup();
    words
});

/// Whether `token` is a reserved built-in word. A bare top-level token equal
/// to one dispatches the built-in; any other token is an argv-first task name.
#[must_use]
pub fn is_reserved(token: &str) -> bool {
    RESERVED.iter().any(|word| word == token)
}

/// The maximum edit distance for a token to be treated as a typo of a reserved
/// built-in verb.
///
/// Looser than the default suggestion distance because the reserved set is
/// small and closed, and the hint is only ever offered *after* the token
/// already failed to resolve as a task — so a slightly wider net (catching
/// `modual` → `modules`) carries little risk of a misleading suggestion.
const RESERVED_SUGGESTION_DISTANCE: usize = 3;

/// The reserved built-in nearest to `token` within
/// `RESERVED_SUGGESTION_DISTANCE`, or `None` when it is not a plausible typo of
/// any built-in.
///
/// Advisory only: the argv-first dispatch never uses this to redirect input —
/// it feeds the "did you mean the built-in?" hint after a token has already
/// failed to resolve as a task. Exact reserved words are handled by dispatch
/// and so are excluded here.
#[must_use]
pub fn nearest_reserved(token: &str) -> Option<&'static str> {
    if is_reserved(token) {
        return None;
    }
    rskit_util::strings::nearest_within(
        token,
        RESERVED.iter().map(String::as_str),
        RESERVED_SUGGESTION_DISTANCE,
    )
}

/// The flags a bare task accepts: the shared execution group plus `-h/--help`,
/// which asks for help instead of running the task.
#[derive(Debug, Parser)]
#[command(
    name = "toven <task>",
    no_binary_name = true,
    disable_help_flag = true,
    disable_version_flag = true,
    args_override_self = true
)]
struct TaskPrefix {
    #[command(flatten)]
    execution: ExecutionArgs,
    #[arg(short = 'h', long = "help")]
    help: bool,
}

/// A parsed `toven <task> [flags] [--] [passthrough]` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskInvocation {
    /// The task name (the first token).
    pub task: String,
    /// The task's own execution flags, before merging with the globals.
    pub flags: ExecutionArgs,
    /// Whether `-h`/`--help` appeared in the flag prefix.
    pub help: bool,
    /// Everything after the prefix (or after `--`), verbatim.
    pub passthrough: Vec<String>,
}

/// Parse the tokens clap captured for a bare task.
///
/// # Errors
/// Returns a typed usage error for an empty token list or an invalid flag
/// value in the prefix.
pub fn parse_task(tokens: &[String]) -> AppResult<TaskInvocation> {
    let (task, rest) = tokens
        .split_first()
        .filter(|(task, _)| !task.is_empty())
        .ok_or_else(|| AppError::invalid_input("command", "expected a task name"))?;
    let schema = FlagSchema::of(&TaskPrefix::command());
    let end = schema.prefix_len(rest);
    let (prefix, tail) = rest.split_at(end);
    let passthrough = match tail.split_first() {
        Some((separator, after)) if separator == "--" => after.to_vec(),
        _ => tail.to_vec(),
    };
    let parsed = TaskPrefix::command()
        .try_get_matches_from(prefix)
        .and_then(|matches| TaskPrefix::from_arg_matches(&matches))
        .map_err(|error| usage_error(&error))?;
    Ok(TaskInvocation {
        task: task.clone(),
        flags: parsed.execution,
        help: parsed.help,
        passthrough,
    })
}

/// Which long and short names the task prefix knows, and whether each takes a
/// value. Read from clap so the scan and the parse cannot disagree.
struct FlagSchema {
    longs: HashMap<String, bool>,
    /// Each short name, with its arg when that arg takes a value.
    shorts: HashMap<char, Option<clap::Arg>>,
}

impl FlagSchema {
    fn of(command: &clap::Command) -> Self {
        let mut longs = HashMap::new();
        let mut shorts = HashMap::new();
        for arg in command.get_arguments() {
            let takes_value = arg.get_action().takes_values();
            for long in arg
                .get_long()
                .into_iter()
                .chain(arg.get_all_aliases().unwrap_or_default())
            {
                longs.insert(long.to_owned(), takes_value);
            }
            for short in arg
                .get_short()
                .into_iter()
                .chain(arg.get_all_short_aliases().unwrap_or_default())
            {
                shorts.insert(short, takes_value.then(|| arg.clone()));
            }
        }
        Self { longs, shorts }
    }

    /// The number of leading tokens that are known flags (with their values).
    /// Stops at `--`, a positional, or the first unknown flag.
    fn prefix_len(&self, tokens: &[String]) -> usize {
        let mut index = 0;
        while let Some(token) = tokens.get(index) {
            let Some(width) = self.flag_width(token) else {
                break;
            };
            // A missing value, or `--` in its place, is left for clap to reject.
            index += match tokens.get(index + 1) {
                Some(next) if width == 2 && next != "--" => 2,
                _ => 1,
            };
        }
        index
    }

    /// `Some(2)` for a known flag whose value is the next token, `Some(1)` for
    /// any other known flag form, `None` when the token ends the prefix.
    fn flag_width(&self, token: &str) -> Option<usize> {
        if token == "--" {
            return None;
        }
        if let Some(body) = token.strip_prefix("--") {
            let (name, inline) = match body.split_once('=') {
                Some((name, _)) => (name, true),
                None => (body, false),
            };
            let takes_value = *self.longs.get(name)?;
            return Some(if takes_value && !inline { 2 } else { 1 });
        }
        let cluster = token
            .strip_prefix('-')
            .filter(|cluster| !cluster.is_empty())?;
        for (offset, short) in cluster.char_indices() {
            if let Some(arg) = self.shorts.get(&short)? {
                let joined = &cluster[offset + short.len_utf8()..];
                if joined.is_empty() {
                    return Some(2);
                }
                return is_joined_value(arg, short, joined).then_some(1);
            }
        }
        Some(1)
    }
}

/// Whether `joined` (the rest of a short cluster after the value-taking flag
/// `short`) is that flag's value. A remainder that starts with a letter and
/// does not parse is a word the task owns, such as `go test -json`; any other
/// remainder is claimed so clap can report a bad value.
fn is_joined_value(arg: &clap::Arg, short: char, joined: &str) -> bool {
    !joined.starts_with(char::is_alphabetic)
        || clap::Command::new("probe")
            .no_binary_name(true)
            .disable_help_flag(true)
            .arg(arg.clone().global(false))
            .try_get_matches_from([format!("-{short}{joined}")])
            .is_ok()
}

/// Lift a clap parse error into a typed usage error, keeping clap's message
/// but dropping its usage block.
fn usage_error(error: &clap::Error) -> AppError {
    let rendered = error.to_string();
    let message = rendered
        .lines()
        .next()
        .unwrap_or_default()
        .trim_start_matches("error: ")
        .to_owned();
    AppError::invalid_input("task flags", message)
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
        // Friction-free passthrough: the first non-Toven token ends the prefix, so a
        // flag Toven does not own becomes the task's own argument.
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
    fn help_in_the_prefix_asks_for_help_instead_of_running_the_task() {
        for flag in ["--help", "-h"] {
            let invocation =
                parse_task(&tokens(&["tidy-fix", "--no-cache", flag])).expect("parses");
            assert!(invocation.help, "{flag}");
            assert!(invocation.passthrough.is_empty());
        }
    }

    #[test]
    fn help_after_the_prefix_or_separator_is_forwarded_to_the_task() {
        let forwarded = parse_task(&tokens(&["test", "--", "--help"])).expect("parses");
        assert!(!forwarded.help);
        assert_eq!(forwarded.passthrough, vec!["--help"]);
        let argument = parse_task(&tokens(&["test", "./...", "-h"])).expect("parses");
        assert!(!argument.help);
        assert_eq!(argument.passthrough, vec!["./...", "-h"]);
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
            "--dependents",
            "--dependencies",
        ]))
        .expect("parses");
        assert_eq!(invocation.flags.module, vec!["rust:core"]);
        assert_eq!(invocation.flags.workspace, vec!["rust"]);
        assert!(invocation.flags.with_dependents);
        assert!(invocation.flags.with_dependencies);
    }

    #[test]
    fn accepts_the_with_dependents_alias_on_a_bare_task() {
        let invocation = parse_task(&tokens(&["test", "--with-dependents"])).expect("parses");
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
    fn parses_jobs_ceiling_on_a_bare_task() {
        let invocation = parse_task(&tokens(&["test", "--jobs", "4"])).expect("parses");
        assert_eq!(invocation.flags.jobs, Some(4));
        let short = parse_task(&tokens(&["test", "-j", "1"])).expect("parses");
        assert_eq!(short.flags.jobs, Some(1));
    }

    #[test]
    fn rejects_a_zero_or_malformed_jobs_value() {
        assert!(parse_task(&tokens(&["test", "--jobs", "0"])).is_err());
        assert!(parse_task(&tokens(&["test", "--jobs", "many"])).is_err());
        assert!(parse_task(&tokens(&["test", "--jobs"])).is_err());
    }

    #[test]
    fn parses_compute_budget_on_a_bare_task() {
        use toven_ports::ComputeBudget;
        let auto = parse_task(&tokens(&["test", "--compute-budget", "auto"])).expect("parses");
        assert_eq!(auto.flags.compute_budget, Some(ComputeBudget::Auto));
        let fixed = parse_task(&tokens(&["test", "--compute-budget", "8"])).expect("parses");
        assert_eq!(fixed.flags.compute_budget, Some(ComputeBudget::fixed(8)));
        let inherit =
            parse_task(&tokens(&["test", "--compute-budget", "inherit"])).expect("parses");
        assert_eq!(inherit.flags.compute_budget, Some(ComputeBudget::Inherit));
    }

    #[test]
    fn rejects_a_malformed_compute_budget_value() {
        assert!(parse_task(&tokens(&["test", "--compute-budget", "turbo"])).is_err());
        assert!(parse_task(&tokens(&["test", "--compute-budget"])).is_err());
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
    fn parses_color_choice_on_a_bare_task() {
        use crate::flags::ColorWhen;
        let invocation = parse_task(&tokens(&["test", "--color", "always"])).expect("parses");
        assert_eq!(invocation.task, "test");
        assert_eq!(invocation.flags.color, Some(ColorWhen::Always));
        // `--color` is a Toven flag, so it is absorbed — never leaked into the task's
        // own passthrough argv.
        assert!(invocation.passthrough.is_empty());
    }

    #[test]
    fn rejects_an_unknown_or_valueless_color_choice() {
        assert!(parse_task(&tokens(&["test", "--color", "sometimes"])).is_err());
        assert!(parse_task(&tokens(&["test", "--color"])).is_err());
    }

    #[test]
    fn parses_view_mode_on_a_bare_task() {
        use crate::flags::ViewMode;
        let invocation = parse_task(&tokens(&["test", "--view", "tiles"])).expect("parses");
        assert_eq!(invocation.task, "test");
        assert_eq!(invocation.flags.view, Some(ViewMode::Tiles));
        // `--view` is a Toven flag, so it is absorbed — never leaked into the task's
        // own passthrough argv.
        assert!(invocation.passthrough.is_empty());
    }

    #[test]
    fn rejects_an_unknown_or_valueless_view_mode() {
        assert!(parse_task(&tokens(&["test", "--view", "grid"])).is_err());
        assert!(parse_task(&tokens(&["test", "--view"])).is_err());
    }

    #[test]
    fn passthrough_separator_is_not_a_flag_value() {
        assert!(parse_task(&tokens(&["test", "--config", "--", "--flag"])).is_err());
        assert!(parse_task(&tokens(&["test", "--output", "--", "--flag"])).is_err());
    }

    #[test]
    fn a_bare_task_accepts_the_same_flag_forms_as_toven_run() {
        let invocation = parse_task(&tokens(&[
            "test",
            "--timeout=5s",
            "-j4",
            "-vv",
            "--output=jsonl",
            "--module=rust:core",
            "-q",
            "./...",
            "-v",
        ]))
        .expect("parses");
        assert_eq!(
            invocation.flags.timeout,
            Some(std::time::Duration::from_secs(5))
        );
        assert_eq!(invocation.flags.jobs, Some(4));
        assert_eq!(invocation.flags.verbose, 2);
        assert_eq!(invocation.flags.quiet, 1);
        assert_eq!(invocation.flags.output, Some(OutputKind::Jsonl));
        assert_eq!(invocation.flags.module, vec!["rust:core"]);
        assert_eq!(invocation.passthrough, vec!["./...", "-v"]);
    }

    #[test]
    fn a_cluster_with_an_unknown_short_ends_the_prefix_verbatim() {
        let invocation = parse_task(&tokens(&["test", "-vx", "--dry-run"])).expect("parses");
        assert_eq!(invocation.flags.verbose, 0);
        assert_eq!(invocation.passthrough, vec!["-vx", "--dry-run"]);
    }

    #[test]
    fn a_bad_joined_value_is_rejected() {
        assert!(parse_task(&tokens(&["test", "--timeout=soon"])).is_err());
        assert!(parse_task(&tokens(&["test", "-j0"])).is_err());
    }

    #[test]
    fn a_short_value_flag_joined_to_letters_is_the_tasks() {
        for cluster in ["-json", "-vjx"] {
            let invocation = parse_task(&tokens(&["test", cluster])).expect("parses");
            assert_eq!(invocation.flags.jobs, None, "{cluster}");
            assert_eq!(invocation.passthrough, vec![cluster]);
        }
        for joined in ["-j4", "-j=4"] {
            let invocation = parse_task(&tokens(&["test", joined])).expect("parses");
            assert_eq!(invocation.flags.jobs, Some(4), "{joined}");
        }
    }

    #[test]
    fn an_empty_task_name_is_rejected() {
        assert!(parse_task(&tokens(&["", "--help"])).is_err());
    }

    #[test]
    fn every_built_in_verb_and_alias_is_reserved() {
        for word in [
            "commit-lint",
            "ls",
            "list",
            "deps",
            "help",
            "run",
            "release",
        ] {
            assert!(is_reserved(word), "{word}");
        }
    }

    #[test]
    fn empty_token_vector_is_rejected() {
        assert!(parse_task(&[]).is_err());
    }
}
