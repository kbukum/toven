//! [`HookRunner`] — the injected seam that runs one configured hook task.

use rskit_errors::AppResult;

use super::HookPhase;

/// Runs a single configured lifecycle hook: the task named `reference`, executed
/// as the given [`HookPhase`] around a verb's mutation.
///
/// The verb owns *when* hooks run (pre before the mutation, post after success)
/// and *fail-closed* semantics (a failing `pre` hook aborts before any
/// mutation); the runner owns *how* one task reference is executed. The concrete
/// runner resolves the reference against the composed task model and runs it
/// through the normal PLAN → APPLY path, so an unknown reference or a non-zero
/// task result surfaces as a typed error.
///
/// Object-safe so a verb can hold `&dyn HookRunner`.
pub trait HookRunner {
    /// Run the task named `reference` as a `phase` hook.
    ///
    /// # Errors
    /// Fails closed when the referenced task is unknown, cannot be planned, or
    /// exits non-zero — the caller must not proceed past a failed `pre` hook.
    fn run_hook(&self, phase: HookPhase, reference: &str) -> AppResult<()>;
}
