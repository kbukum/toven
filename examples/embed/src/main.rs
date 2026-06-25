//! Example: embedding Toven in a custom binary.
//!
//! A third party can build its own front-end by selecting an adapter set and
//! delegating dispatch to [`toven_cli::run`] — exactly what the first-party apps
//! do. This minimal sample wires a single Rust provider; it exists so the
//! `examples/*` workspace members stay compiled under `--workspace` and the
//! embedding contract can't bitrot.

use std::process::ExitCode;

use toven_ports::Provider;
use toven_rust::RustProvider;

fn main() -> ExitCode {
    let provider = match RustProvider::new() {
        Ok(provider) => provider,
        Err(error) => {
            eprintln!("error: failed to initialize the Rust adapter: {error}");
            return ExitCode::FAILURE;
        }
    };
    let providers: Vec<&dyn Provider> = vec![&provider];
    let code = toven_cli::run(&providers);
    ExitCode::from(u8::try_from(code.as_i32()).unwrap_or(1))
}
