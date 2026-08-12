//! The [`HookRunner`] port: run a configured lifecycle hook (a task reference)
//! around **any** unit's mutation.
//!
//! Hooks are a **unit-agnostic** seam. The dispatch driver resolves a unit's
//! composed [`HooksConfig`](crate::config::HooksConfig) references (from the
//! project-level `[hooks.<unit>]` map) and asks the injected [`HookRunner`] to
//! run each — before the mutation for [`HookPhase::Before`], after a successful
//! mutation for [`HookPhase::After`]. A third phase,
//! [`HookPhase::OnResolved`], runs *inside* a unit's mutation once its decision
//! is resolved but before it is staged (the bump seam, handed the authoritative
//! post-bump version map so its edits join the staged set) — the same runner,
//! one more phase. The port hides *how* a task reference is executed (planning +
//! APPLY) from the unit, so a unit stays free of the async task-execution stack
//! and any unit — task, native capability, or composite — reuses the one runner.

mod phase;
mod runner;

pub use phase::HookPhase;
pub use runner::HookRunner;
