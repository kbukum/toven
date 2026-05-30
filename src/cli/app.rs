//! Public CLI entrypoints.

use std::process::ExitCode;

use clap::Command;

/// Build the Toven command.
#[must_use]
pub fn command() -> Command {
    crate::cli::commands::command()
}

/// Run the CLI.
pub fn run() -> ExitCode {
    crate::cli::dispatch::run()
}
