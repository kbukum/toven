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
//!
//! The bump verb additionally carries a mid-mutation seam,
//! [`ResolvedHookRunner`]: an `on-resolved` task run after the version decision
//! and native version-reference sync but before staging, handed the
//! authoritative post-bump version map so its edits join the staged set.

mod phase;
mod resolved;
mod runner;

pub use phase::HookPhase;
pub use resolved::ResolvedHookRunner;
pub use runner::HookRunner;
