//! `toven completions <shell>`: emit a shell completion script.
//!
//! Generates a static completion script for the derived clap command tree —
//! reserved verbs, their subcommands, and global flags — for the user to install
//! into their shell. It prints before any project load (like `--help`), so it
//! needs no `toven.toml`. The script goes to stdout (machine-consumable); the CLI
//! never rewrites user argv, so completion is purely advisory.

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
