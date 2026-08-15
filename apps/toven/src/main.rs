//! `toven` — the umbrella CLI bundling every first-party adapter.
//!
//! Explicit constructor wiring (no DI container): build the Rust, Go, and
//! command providers, hand the bundled set to the shared [`toven_cli::run`]
//! dispatcher, and exit with the resulting process code. A repository selects
//! whichever ecosystems it declares in `toven.toml`; unused adapters simply
//! contribute no configured sections.

use std::process::ExitCode;
use std::sync::Arc;

use rskit_cli::ExitCode as CliExit;
use toven_command::CommandProvider;
use toven_exec::{LifecyclePolicy, ProcessSupervisor, ProcessToolRunner};
use toven_go::GoProvider;
use toven_ports::{Provider, ToolRunner};
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
    // One shared process supervisor for the whole run: the provider tool runner
    // (PLAN discovery, e.g. `cargo metadata`) and the CLI's toolchain-prober and
    // APPLY runners all register their spawned children with it, so a stop signal
    // reaps every child through a single process-level backstop.
    let supervisor = Arc::new(ProcessSupervisor::new(LifecyclePolicy::default()));
    let runner: Arc<dyn ToolRunner> =
        Arc::new(ProcessToolRunner::new().with_supervisor(Arc::clone(&supervisor)));
    let rust = RustProvider::new(runner.clone())?;
    let go = GoProvider::new(runner)?;
    let command = CommandProvider::new()?;
    let providers: Vec<&dyn Provider> = vec![&rust, &go, &command];
    Ok(toven_cli::run(&providers, &supervisor))
}
