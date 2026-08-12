//! The [`HookRunner`] port: run a configured lifecycle hook (a task reference)
//! around **any** unit's mutation.
//!
//! Hooks are a **unit-agnostic** seam. The dispatch driver resolves a unit's
//! composed [`HooksConfig`](crate::config::HooksConfig) references (from the
//! project-level `[hooks.<verb>]` map) and asks the injected [`HookRunner`] to
//! run each through a typed [`HookInvocation`] — before the mutation, after a
//! successful mutation, or inside bump resolution with the authoritative
//! post-bump version map. The port hides *how* a task reference is executed
//! (planning + APPLY) from the unit, so a unit stays free of the async
//! task-execution stack and any unit — task, native capability, or composite —
//! reuses the one runner.

mod invocation;
mod phase;
mod runner;

pub use invocation::HookInvocation;
pub use phase::HookPhase;
pub use runner::HookRunner;
