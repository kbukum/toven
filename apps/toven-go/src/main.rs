//! `toven-go` — Toven specialized for Go workspaces.
//!
//! Explicit constructor wiring (no DI container): build the Go provider, hand
//! it to the shared [`toven_cli::run`] dispatcher, and exit with the resulting
//! process code.

use std::process::ExitCode;

use rskit_cli::ExitCode as CliExit;
use toven_go::GoProvider;
use toven_ports::Provider;

fn main() -> ExitCode {
    let code = match wire_and_run() {
        Ok(code) => code,
        Err(error) => toven_cli::report_error(&error),
    };
    ExitCode::from(u8::try_from(code.as_i32()).unwrap_or(1))
}

/// Build the Go provider and run the CLI, returning the process code.
fn wire_and_run() -> rskit_errors::AppResult<CliExit> {
    let provider = GoProvider::new()?;
    let providers: Vec<&dyn Provider> = vec![&provider];
    Ok(toven_cli::run(&providers))
}
