//! The injected process-execution seams.
//!
//! Two coherent runner ports share the argv-first invocation vocabulary:
//!
//! - [`CommandRunner`] — the async, streaming, cancellable, persistent-aware
//!   seam the APPLY wave walk drives. Normal invocations produce a
//!   [`RunOutcome`]; persistent ones produce a [`StartOutcome`] handing back a
//!   [`HeldProcess`] the engine holds until teardown.
//! - [`ToolRunner`] — the synchronous one-shot seam every "spawn one argv-first
//!   tool, forward named secrets, gate on its exit" call site runs through
//!   ([`ToolInvocation`] → [`ToolOutcome`]).
//!
//! The concrete `rskit-process`-backed runners live in the engine; scriptable
//! doubles live in `toven-testkit`.

mod environment;
mod invocation;
mod outcome;
mod output;
mod runner;
mod tool;

pub use environment::{InvocationEnvPolicy, InvocationEnvironment};
pub use invocation::Invocation;
pub use outcome::{HeldProcess, RunOutcome, StartOutcome};
pub use output::OutputObserver;
pub use runner::CommandRunner;
pub use tool::{ForwardEnvAs, ToolInvocation, ToolOutcome, ToolRunner, Truncation};
