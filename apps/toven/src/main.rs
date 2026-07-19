//! `toven` — the umbrella CLI bundling every first-party adapter.
//!
//! Explicit constructor wiring (no DI container): build the Rust, Go, and
//! command providers, hand the bundled set to the shared [`toven_cli::run`]
//! dispatcher, and exit with the resulting process code. A repository selects
//! whichever ecosystems it declares in `toven.toml`; unused adapters simply
//! contribute no configured sections.

use std::process::ExitCode;

use rskit_cli::ExitCode as CliExit;
use toven_command::CommandProvider;
use toven_go::GoProvider;
use toven_ports::Provider;
use toven_rust::RustProvider;

fn main() -> ExitCode {
    let code = match wire_and_run() {
        Ok(code) => code,
        Err(error) => toven_cli::report_error(&error),
    };
    ExitCode::from(u8::try_from(code.as_i32()).unwrap_or(1))
}

/// Build the bundled provider set and run the CLI, returning the process code.
fn wire_and_run() -> rskit_errors::AppResult<CliExit> {
    let rust = RustProvider::new()?;
    let go = GoProvider::new()?;
    let command = CommandProvider::new()?;
    let providers: Vec<&dyn Provider> = vec![&rust, &go, &command];
    Ok(toven_cli::run(&providers))
}
