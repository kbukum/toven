//! [`ResolvedHookRunner`] — the bump-scoped mid-mutation seam that runs one
//! `on-resolved` task, handed the authoritative post-bump version map.

use std::path::Path;

use rskit_errors::AppResult;

/// Runs one bump `on-resolved` hook: the task named `reference`, handed the
/// authoritative post-bump version map.
///
/// The seam runs **after** the version decision and native version-reference
/// sync but **before** the mutation is staged, so its file edits can join the
/// same staged set as the manifests. Unlike the whole-verb
/// [`HookRunner`](super::HookRunner) (`pre` before the
/// mutation, `post` after it succeeds), this seam runs *inside* the bump
/// mutation with the resolved `module → post-bump version` map already known.
/// The map is materialized as a generated file and its path is handed to the
/// task as an argv-first argument (no implicit shell), so the task can rewrite
/// arbitrary related files the native sync does not cover. The concrete runner
/// resolves the reference against the composed task model and runs it through
/// the normal PLAN → APPLY path, so an unknown reference or a non-zero task
/// result surfaces as a typed error.
///
/// Object-safe so the engine can hold `&dyn ResolvedHookRunner`.
pub trait ResolvedHookRunner {
    /// Run the task named `reference` as a bump `on-resolved` hook, handing it
    /// `version_map` — the path to the generated authoritative version map.
    ///
    /// # Errors
    /// Fails closed when the referenced task is unknown, cannot be planned, or
    /// exits non-zero — the caller must abort the bump and restore every already
    /// mutated member, staging nothing.
    fn run_resolved(&self, reference: &str, version_map: &Path) -> AppResult<()>;
}
