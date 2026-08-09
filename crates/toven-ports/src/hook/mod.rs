//! The [`HookRunner`] port: run a configured lifecycle hook (a task reference)
//! around a verb's mutation.
//!
//! Hooks are a **verb-agnostic** seam. The CLI dispatch driver resolves a verb's
//! composed [`HooksConfig`](crate::config::HooksConfig) references (from the
//! project-level `[hooks.<verb>]` map) and asks the injected [`HookRunner`] to
//! run each — before the mutation for [`HookPhase::Pre`], after a successful
//! mutation for [`HookPhase::Post`]. The port hides *how* a task reference is
//! executed (planning + APPLY) from the verb, so the verb stays free of the
//! async task-execution stack and any verb can reuse the same runner.

mod phase;
mod runner;

pub use phase::HookPhase;
pub use runner::HookRunner;
