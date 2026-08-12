//! [`HookRunner`] — the one injected seam that runs a configured lifecycle hook
//! around any unit's mutation.

use rskit_errors::AppResult;

use super::HookInvocation;

/// Runs a single configured lifecycle hook: the task named `reference`, executed
/// as the given [`HookInvocation`] around a unit's mutation.
///
/// This is the **one** hook mechanism for the whole system: it wraps any unit —
/// a task ([`Argv`](toven_model::Backing::Argv)), a native capability
/// ([`Native`](toven_model::Backing::Native): bump/tag/publish/…), or a
/// delegated tool — identically, differing only in phase. The unit owns *when*
/// hooks run and *fail-closed* semantics (a failing
/// [`Before`](HookInvocation::Before) hook aborts before any mutation); the
/// runner owns *how* one task reference is executed. The concrete runner
/// resolves the reference against the composed task model and runs it through
/// the normal PLAN → APPLY path, so an unknown reference or a non-zero task
/// result surfaces as a typed error.
///
/// [`HookInvocation`] encodes each phase together with any required payload, so
/// callers cannot omit an on-resolved version map or attach one to before/after.
/// The map path is handed to the task argv-first (no implicit shell), so the task
/// can rewrite related files the native sync does not cover and its edits join
/// the same staged set.
///
/// Object-safe so a unit can hold `&dyn HookRunner`.
pub trait HookRunner {
    /// Run the task named `reference` for `invocation`.
    ///
    /// # Errors
    /// Fails closed when the referenced task is unknown, cannot be planned, or
    /// exits non-zero — the caller must not proceed past a failed
    /// [`Before`](HookInvocation::Before)/
    /// [`OnResolved`](HookInvocation::OnResolved) hook, and an
    /// [`OnResolved`](HookInvocation::OnResolved) failure additionally requires
    /// the caller to restore every already-mutated unit, staging nothing.
    fn run_hook(&self, invocation: HookInvocation<'_>, reference: &str) -> AppResult<()>;
}
