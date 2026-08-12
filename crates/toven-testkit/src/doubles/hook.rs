//! Shared hook doubles: [`RecordingHookRunner`] (whole-unit before/after) and
//! [`ScriptedResolvedRunner`] (the bump `on-resolved` mid-mutation seam) — both
//! implementing the one [`HookRunner`] port.
//!
//! Unit tests (release first) inject these instead of the real PLAN→APPLY hook
//! runner: [`RecordingHookRunner`] records every `(phase, reference)` it is
//! asked to run so a test can assert ordering and fail-closed semantics (a
//! failing `before` hook must abort before any mutation), and it can be scripted
//! to fail on a chosen reference. [`ScriptedResolvedRunner`] does the same for
//! the [`HookPhase::OnResolved`] seam and can additionally produce working-tree
//! edits the engine then re-stages. Both are `Clone` (shared state) so a test
//! can hold a handle for assertions after injecting it.

use std::path::Path;
use std::sync::{Arc, Mutex};

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_ports::{ChangeRecord, HookPhase, HookRunner};

/// A single hook invocation recorded by [`RecordingHookRunner`].
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct HookCall {
    /// The phase the hook was run as.
    pub phase: HookPhase,
    /// The task reference that was run.
    pub reference: String,
}

#[derive(Debug, Default)]
struct RecordingHookRunnerState {
    calls: Vec<HookCall>,
    fail_on: Option<String>,
}

/// A [`HookRunner`] that records its calls, or fails on a scripted reference.
#[derive(Debug, Clone, Default)]
pub struct RecordingHookRunner {
    inner: Arc<Mutex<RecordingHookRunnerState>>,
}

impl RecordingHookRunner {
    /// A runner that records and succeeds for every reference.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A runner that fails closed when asked to run `reference` (any phase), so
    /// the fail-closed abort path is exercised offline.
    #[must_use]
    pub fn failing_on(reference: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(RecordingHookRunnerState {
                calls: Vec::new(),
                fail_on: Some(reference.into()),
            })),
        }
    }

    /// The calls recorded so far, in invocation order.
    #[must_use]
    pub fn calls(&self) -> Vec<HookCall> {
        self.state().calls.clone()
    }

    /// The recorded references for `phase`, in invocation order.
    #[must_use]
    pub fn references(&self, phase: HookPhase) -> Vec<String> {
        self.state()
            .calls
            .iter()
            .filter(|call| call.phase == phase)
            .map(|call| call.reference.clone())
            .collect()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, RecordingHookRunnerState> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl HookRunner for RecordingHookRunner {
    fn run_hook(
        &self,
        phase: HookPhase,
        reference: &str,
        _version_map: Option<&Path>,
    ) -> AppResult<()> {
        // Record the attempt before failing so a test can assert a failing
        // `before` hook was reached (and that nothing after it ran).
        self.state().calls.push(HookCall {
            phase,
            reference: reference.to_string(),
        });
        let fail_on = self.state().fail_on.clone();
        if fail_on.as_deref() == Some(reference) {
            return Err(AppError::new(
                ErrorCode::Internal,
                format!("scripted hook failure for '{reference}'"),
            ));
        }
        Ok(())
    }
}

/// A single bump `on-resolved` invocation recorded by
/// [`ScriptedResolvedRunner`].
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ResolvedCall {
    /// The task reference that was run.
    pub reference: String,
    /// The handed-off version-map file path.
    pub version_map: std::path::PathBuf,
    /// The version-map file contents read back at invocation time (the JSON the
    /// engine materialized), so a test can assert the task received the
    /// authoritative map.
    pub version_map_contents: String,
}

#[derive(Debug, Default)]
struct ScriptedResolvedRunnerState {
    calls: Vec<ResolvedCall>,
    fail_on: Option<String>,
    produces: Vec<ChangeRecord>,
    worktree: Option<Arc<Mutex<Vec<ChangeRecord>>>>,
}

/// A [`HookRunner`] double for the bump [`HookPhase::OnResolved`] seam.
///
/// It records each `(reference, version_map path, map contents)` it is asked to
/// run, can be scripted to fail on a chosen reference (exercising the
/// restore-on-failure abort path offline), and — modelling a task that edits
/// files — can push scripted [`ChangeRecord`]s into a shared working-tree handle
/// on success, so the engine's post-hook re-stage collection observes them. It
/// is `Clone` (shared state) so a test can keep a handle for assertions after
/// injecting it.
#[derive(Debug, Clone, Default)]
pub struct ScriptedResolvedRunner {
    inner: Arc<Mutex<ScriptedResolvedRunnerState>>,
}

impl ScriptedResolvedRunner {
    /// A runner that records and succeeds for every reference, producing no
    /// working-tree edits.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A runner that, on success, appends `produces` to the `worktree` handle —
    /// modelling an `on-resolved` task that edits those paths, which the engine
    /// then re-stages.
    #[must_use]
    pub fn producing(worktree: Arc<Mutex<Vec<ChangeRecord>>>, produces: Vec<ChangeRecord>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ScriptedResolvedRunnerState {
                calls: Vec::new(),
                fail_on: None,
                produces,
                worktree: Some(worktree),
            })),
        }
    }

    /// A runner that fails closed when asked to run `reference`, so the
    /// abort-and-restore path is exercised offline.
    #[must_use]
    pub fn failing_on(reference: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ScriptedResolvedRunnerState {
                calls: Vec::new(),
                fail_on: Some(reference.into()),
                produces: Vec::new(),
                worktree: None,
            })),
        }
    }

    /// A runner that appends `produces` to the `worktree` handle and *then*
    /// fails on `reference` — modelling an `on-resolved` task that creates
    /// working-tree files before erroring, so the abort path's untracked-file
    /// cleanup is exercised.
    #[must_use]
    pub fn producing_then_failing(
        worktree: Arc<Mutex<Vec<ChangeRecord>>>,
        produces: Vec<ChangeRecord>,
        reference: impl Into<String>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ScriptedResolvedRunnerState {
                calls: Vec::new(),
                fail_on: Some(reference.into()),
                produces,
                worktree: Some(worktree),
            })),
        }
    }

    /// The calls recorded so far, in invocation order.
    #[must_use]
    pub fn calls(&self) -> Vec<ResolvedCall> {
        self.state().calls.clone()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, ScriptedResolvedRunnerState> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl HookRunner for ScriptedResolvedRunner {
    fn run_hook(
        &self,
        phase: HookPhase,
        reference: &str,
        version_map: Option<&Path>,
    ) -> AppResult<()> {
        // This double models only the mid-mutation on-resolved seam, which
        // always hands over a materialized version map. A missing phase/payload
        // is a wiring bug in the test, so surface it as a typed error rather
        // than masquerading as a no-op.
        let (HookPhase::OnResolved, Some(version_map)) = (phase, version_map) else {
            return Err(AppError::new(
                ErrorCode::Internal,
                format!(
                    "ScriptedResolvedRunner only models the on-resolved seam; got phase {} with{} a version map",
                    phase.as_str(),
                    if version_map.is_some() { "" } else { "out" },
                ),
            ));
        };
        // Read the handed-off map back so a test can assert the task received
        // the authoritative version map materialized by the engine. Fail closed
        // if the map is missing or unreadable, so a bad handoff (e.g. the engine
        // passing an invalid path) surfaces instead of masquerading as empty.
        let version_map_contents = std::fs::read_to_string(version_map).map_err(|source| {
            AppError::new(
                ErrorCode::Internal,
                format!(
                    "on-resolved runner could not read handed-off version map '{}': {source}",
                    version_map.display()
                ),
            )
        })?;
        let (fail, produces, worktree) = {
            let mut state = self.state();
            state.calls.push(ResolvedCall {
                reference: reference.to_string(),
                version_map: version_map.to_path_buf(),
                version_map_contents,
            });
            let fail = state.fail_on.as_deref() == Some(reference);
            (fail, state.produces.clone(), state.worktree.clone())
        };
        // Apply the produced edits *before* an optional failure so a
        // produce-then-fail task leaves working-tree files the abort path must
        // clean up.
        if let Some(worktree) = &worktree {
            worktree
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend(produces);
        }
        if fail {
            return Err(AppError::new(
                ErrorCode::Internal,
                format!("scripted on-resolved failure for '{reference}'"),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{HookPhase, HookRunner, RecordingHookRunner};

    #[test]
    fn one_runner_records_before_on_resolved_and_after_through_one_method() {
        // Proves the single HookRunner mechanism spans every lifecycle phase:
        // one runner, one `run_hook`, three phases — no phase-specific trait.
        let runner = RecordingHookRunner::new();
        runner
            .run_hook(HookPhase::Before, "gate", None)
            .expect("before hook runs");
        runner
            .run_hook(HookPhase::OnResolved, "sync", None)
            .expect("on-resolved hook runs");
        runner
            .run_hook(HookPhase::After, "notify", None)
            .expect("after hook runs");

        let calls = runner.calls();
        let phases: Vec<HookPhase> = calls.iter().map(|call| call.phase).collect();
        assert_eq!(
            phases,
            [HookPhase::Before, HookPhase::OnResolved, HookPhase::After]
        );
        let references: Vec<&str> = calls.iter().map(|call| call.reference.as_str()).collect();
        assert_eq!(references, ["gate", "sync", "notify"]);
    }
}
