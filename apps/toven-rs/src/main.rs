//! `toven-rs` — Toven specialized for Rust workspaces.
//!
//! Explicit constructor wiring (no DI container): build the Rust provider, hand
//! it to the shared [`toven_cli::run`] dispatcher, and exit with the resulting
//! process code. Adding or swapping ecosystems is a one-line change here.

use std::process::ExitCode;
use std::sync::Arc;

use rskit_cli::ExitCode as CliExit;
use toven_exec::{LifecyclePolicy, ProcessSupervisor, ProcessToolRunner};
use toven_ports::{Provider, ToolRunner};
use toven_rust::RustProvider;

fn main() -> ExitCode {
    let code = match wire_and_run() {
        Ok(code) => code,
        Err(error) => toven_cli::report_error(&error),
    };
    ExitCode::from(u8::try_from(code.as_i32()).unwrap_or(1))
}

/// Build the Rust provider and run the CLI, returning the process code.
fn wire_and_run() -> rskit_errors::AppResult<CliExit> {
    // One shared process supervisor for the whole run: the provider tool runner
    // and the CLI's toolchain-prober and APPLY runners all register spawned
    // children with it, so a stop signal reaps every child through one backstop.
    let supervisor = Arc::new(ProcessSupervisor::new(LifecyclePolicy::default()));
    let runner: Arc<dyn ToolRunner> =
        Arc::new(ProcessToolRunner::new().with_supervisor(Arc::clone(&supervisor)));
    let provider = RustProvider::new(runner)?;
    let providers: Vec<&dyn Provider> = vec![&provider];
    Ok(toven_cli::run(&providers, &supervisor))
}
