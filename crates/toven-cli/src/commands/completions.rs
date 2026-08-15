//! `toven completions <shell>`: emit a shell completion script.
//!
//! Generates a static completion script for the derived clap command tree —
//! reserved verbs, their subcommands, and global flags — for the user to
//! install into their shell. It prints before any project load (like `--help`),
//! so it needs no `toven.toml`. The script goes to stdout (machine-consumable);
//! the CLI never rewrites user argv, so completion is purely advisory.

use std::io;

use clap::CommandFactory;
use clap_complete::{Shell, generate};
use rskit_cli::ExitCode;

use crate::flags::Cli;

/// The program name completions are generated for.
const PROGRAM: &str = "toven";

/// `toven completions <bash|zsh|fish|powershell|elvish>`.
///
/// Always succeeds: clap validates the shell before dispatch, so an unsupported
/// value is already rejected as a usage error upstream.
pub(crate) fn completions(shell: Shell) -> ExitCode {
    let mut command = Cli::command();
    command.set_bin_name(PROGRAM);
    generate(shell, &mut command, PROGRAM, &mut io::stdout());
    ExitCode::Success
}

#[cfg(test)]
mod tests {
    use super::PROGRAM;
    use clap::CommandFactory;
    use clap_complete::{Shell, generate};

    use crate::flags::Cli;

    /// Generate a completion script into a string (the testable core of
    /// [`completions`](super::completions), which writes the same bytes to
    /// stdout).
    fn script_for(shell: Shell) -> String {
        let mut command = Cli::command();
        command.set_bin_name(PROGRAM);
        let mut buffer: Vec<u8> = Vec::new();
        generate(shell, &mut command, PROGRAM, &mut buffer);
        String::from_utf8(buffer).expect("completion scripts are valid UTF-8")
    }

    #[test]
    fn completions_cover_the_full_verb_and_action_tree_for_every_shell() {
        // Completions are derived from the clap command tree, so every verb and release
        // action appears for each supported shell — a regression that dropped one from
        // the tree would surface here.
        for shell in [
            Shell::Bash,
            Shell::Zsh,
            Shell::Fish,
            Shell::PowerShell,
            Shell::Elvish,
        ] {
            let script = script_for(shell);
            for token in [
                "coverage",
                "commit-lint",
                "release",
                "readiness",
                "sbom",
                "depgraphs",
                "federation",
                "driver",
                "cache",
                "completions",
            ] {
                let preview: String = script.chars().take(200).collect();
                assert!(
                    script.contains(token),
                    "{shell} completions omit `{token}` (script is {} bytes; head: {preview:?})",
                    script.len()
                );
            }
        }
    }
}
