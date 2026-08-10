//! `toven-exec` — the one concrete execution home.
//!
//! Layer 1.5 of the hexagonal stack: a focused utility crate that sits directly
//! on the ports ([`toven_ports`]) and the rskit process port, below every
//! engine crate. It owns every concrete subprocess runner plus the shared
//! spawn-lowering they compose:
//!
//! - [`ProcessToolRunner`] — the single `rskit-process`-backed
//!   [`ToolRunner`](toven_ports::ToolRunner) adapter every downward crate
//!   (release, engine, cli) drives one-shot tools through, so the
//!   spawn/capture/timeout/secret policy is decided once, not per call site.
//! - [`ProcessCommandRunner`] — the async streaming
//!   [`CommandRunner`](toven_ports::CommandRunner) the engine APPLY walk drives
//!   each wave unit through (streaming/cancellable capture, the `fail_if_output`
//!   gate), plus the persistent-process spawn helper backing
//!   [`start_persistent`](toven_ports::CommandRunner::start_persistent). It
//!   returns a [`HeldProcess`](toven_ports::HeldProcess) the engine wraps; the
//!   held-set and teardown orchestration around it stays in the engine APPLY
//!   walk.
//! - the shared **argv→[`ProcessSpec`](rskit_process::ProcessSpec) lowering**
//!   ([`spec`]) every runner shape composes, so the argv guard, env-policy
//!   mapping, and named-secret forwarding exist in exactly one place.
//!
//! It holds **no** PLAN/engine logic and **no** copy of the invocation
//! vocabulary — [`InvocationEnvironment`](toven_ports::InvocationEnvironment) /
//! [`InvocationEnvPolicy`](toven_ports::InvocationEnvPolicy) stay owned by
//! [`toven_ports`]; this crate depends up-the-trait for them and owns only the
//! lowering to `rskit-process`.
#![warn(missing_docs)]

mod command;
pub mod spec;
mod tool;

pub use command::ProcessCommandRunner;
pub use tool::ProcessToolRunner;
