//! [`HookRunner`] — the one injected seam that runs a configured lifecycle hook
//! around any unit's mutation.

use std::path::Path;

use rskit_errors::AppResult;

use super::HookPhase;

/// Runs a single configured lifecycle hook: the task named `reference`, executed
/// as the given [`HookPhase`] around a unit's mutation.
///
/// This is the **one** hook mechanism for the whole system: it wraps any unit —
/// a task ([`Argv`](toven_model::Backing::Argv)), a native capability
/// ([`Native`](toven_model::Backing::Native): bump/tag/publish/…), or a
/// delegated tool — identically, differing only in phase. The unit owns *when*
/// hooks run and *fail-closed* semantics (a failing [`Before`](HookPhase::Before)
/// hook aborts before any mutation); the runner owns *how* one task reference is
/// executed. The concrete runner resolves the reference against the composed
/// task model and runs it through the normal PLAN → APPLY path, so an unknown
/// reference or a non-zero task result surfaces as a typed error.
///
/// `version_map` carries the phase's payload: for
/// [`OnResolved`](HookPhase::OnResolved) — the mid-mutation seam that runs once
/// the unit's decision is resolved but before it is staged — it is
/// `Some(path)`, the authoritative resolved version map materialized to a
/// generated file whose path is handed to the task argv-first (no implicit
/// shell), so the task can rewrite related files the native sync does not cover
/// and its edits join the same staged set. For
/// [`Before`](HookPhase::Before)/[`After`](HookPhase::After) it is `None`.
///
/// Object-safe so a unit can hold `&dyn HookRunner`.
pub trait HookRunner {
    /// Run the task named `reference` as a `phase` hook, handed the phase's
    /// `version_map` payload (`Some` only for [`HookPhase::OnResolved`]).
    ///
    /// # Errors
    /// Fails closed when the referenced task is unknown, cannot be planned, or
    /// exits non-zero — the caller must not proceed past a failed
    /// [`Before`](HookPhase::Before)/[`OnResolved`](HookPhase::OnResolved) hook,
    /// and an [`OnResolved`](HookPhase::OnResolved) failure additionally requires
    /// the caller to restore every already-mutated unit, staging nothing.
    fn run_hook(
        &self,
        phase: HookPhase,
        reference: &str,
        version_map: Option<&Path>,
    ) -> AppResult<()>;
}
