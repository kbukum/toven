//! [`CommandRunner`] — the injected process-execution port (the APPLY seam).
//!
//! The wave walk runs resolved [`Invocation`]s through a `&dyn CommandRunner`;
//! the concrete `rskit-process`-backed runner lives in the engine, and a
//! scriptable double lives in `toven-testkit`. Normal invocations produce a
//! [`RunOutcome`]; persistent ones produce a [`StartOutcome`] handing back a
//! [`HeldProcess`] the engine holds until teardown.

mod environment;
mod invocation;
mod outcome;
mod output;
mod runner;

pub use environment::{InvocationEnvPolicy, InvocationEnvironment};
pub use invocation::Invocation;
pub use outcome::{HeldProcess, RunOutcome, StartOutcome};
pub use output::OutputObserver;
pub use runner::CommandRunner;
