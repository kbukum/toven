//! `toven-js` — Toven specialized for JavaScript/TypeScript workspaces.
//!
//! Explicit constructor wiring (no DI container): build the js provider, hand
//! it to the shared [`toven_cli::run`] dispatcher, and exit with the resulting
//! process code.

use std::process::ExitCode;
use std::sync::Arc;

use rskit_cli::ExitCode as CliExit;
use toven_exec::{LifecyclePolicy, ProcessSupervisor, ProcessToolRunner};
use toven_js::JsProvider;
use toven_ports::{Provider, ToolRunner};

fn main() -> ExitCode {
    let code = match wire_and_run() {
        Ok(code) => code,
        Err(error) => toven_cli::report_error(&error),
    };
    ExitCode::from(u8::try_from(code.as_i32()).unwrap_or(1))
}

/// Build the js provider and run the CLI, returning the process code.
fn wire_and_run() -> rskit_errors::AppResult<CliExit> {
    // One shared process supervisor: the release target's runner (pack,
    // publish, lockfile refresh) and the CLI's toolchain probes and APPLY
    // children all register with it, so a stop signal reaps every child
    // through one backstop. js discovery spawns nothing.
    let supervisor = Arc::new(ProcessSupervisor::new(LifecyclePolicy::default()));
    let runner: Arc<dyn ToolRunner> =
        Arc::new(ProcessToolRunner::new().with_supervisor(Arc::clone(&supervisor)));
    let provider = JsProvider::new(runner)?;
    let providers: Vec<&dyn Provider> = vec![&provider];
    Ok(toven_cli::run(&providers, &supervisor))
}
