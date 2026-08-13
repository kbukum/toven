//! Shared [`HookRunner`] port double: [`RecordingHookRunner`].
//!
//! Unit tests (release first) inject these instead of the real PLAN→APPLY hook
//! runner: [`RecordingHookRunner`] records every phase, reference, and optional
//! version-map path so a test can assert payload handoff, ordering, and
//! fail-closed semantics (a failing `before` hook must abort before any
//! mutation), and it can be scripted to fail on a chosen reference.
//! The same double can model the [`HookPhase::OnResolved`] seam and produce
//! working-tree edits the engine then re-stages. It is `Clone` (shared state)
//! so a test can hold a handle for assertions after injecting it.

use std::path::Path;
use std::sync::{Arc, Mutex};

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_ports::{ChangeRecord, HookInvocation, HookPhase, HookRunner};

/// A single hook invocation recorded by [`RecordingHookRunner`].
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct HookCall {
    /// The phase the hook was run as.
    pub phase: HookPhase,
    /// The task reference that was run.
    pub reference: String,
    /// The handed-off version-map path for an on-resolved invocation.
    pub version_map: Option<std::path::PathBuf>,
}

#[derive(Debug, Default)]
struct RecordingHookRunnerState {
    calls: Vec<HookCall>,
    resolved_calls: Vec<ResolvedCall>,
    fail_on: Option<String>,
    produces: Vec<ChangeRecord>,
    worktree: Option<Arc<Mutex<Vec<ChangeRecord>>>>,
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
                resolved_calls: Vec::new(),
                fail_on: Some(reference.into()),
                produces: Vec::new(),
                worktree: None,
            })),
        }
    }

    /// A runner that appends `produces` to `worktree` after an on-resolved
    /// invocation succeeds.
    #[must_use]
    pub fn producing(worktree: Arc<Mutex<Vec<ChangeRecord>>>, produces: Vec<ChangeRecord>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(RecordingHookRunnerState {
                calls: Vec::new(),
                resolved_calls: Vec::new(),
                fail_on: None,
                produces,
                worktree: Some(worktree),
            })),
        }
    }

    /// A runner that produces edits and then fails on `reference`.
    #[must_use]
    pub fn producing_then_failing(
        worktree: Arc<Mutex<Vec<ChangeRecord>>>,
        produces: Vec<ChangeRecord>,
        reference: impl Into<String>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(RecordingHookRunnerState {
                calls: Vec::new(),
                resolved_calls: Vec::new(),
                fail_on: Some(reference.into()),
                produces,
                worktree: Some(worktree),
            })),
        }
    }

    /// The calls recorded so far, in invocation order.
    #[must_use]
    pub fn calls(&self) -> Vec<HookCall> {
        self.state().calls.clone()
    }

    /// The on-resolved calls recorded so far, in invocation order.
    #[must_use]
    pub fn resolved_calls(&self) -> Vec<ResolvedCall> {
        self.state().resolved_calls.clone()
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
    fn run_hook(&self, invocation: HookInvocation<'_>, reference: &str) -> AppResult<()> {
        // Record the attempt before failing so a test can assert a failing
        // `before` hook was reached (and that nothing after it ran).
        self.state().calls.push(HookCall {
            phase: invocation.phase(),
            reference: reference.to_string(),
            version_map: invocation.version_map().map(Path::to_path_buf),
        });
        // Capture the handed-off version map only when the engine actually
        // materialized it. A caller that drives the runner purely to record
        // invocations may pass a path without writing a file; that is not a
        // failure. A file that exists but cannot be read still propagates.
        let version_map_contents = match invocation.version_map() {
            Some(path) if path.exists() => Some(rskit_fs::sync_io::file::read_string(path)?),
            _ => None,
        };
        let (fail, produces, worktree) = {
            let mut state = self.state();
            if let (Some(version_map), Some(version_map_contents)) =
                (invocation.version_map(), version_map_contents)
            {
                state.resolved_calls.push(ResolvedCall {
                    reference: reference.to_string(),
                    version_map: version_map.to_path_buf(),
                    version_map_contents,
                });
            }
            (
                state.fail_on.as_deref() == Some(reference),
                state.produces.clone(),
                state.worktree.clone(),
            )
        };
        if invocation.phase() == HookPhase::OnResolved
            && let Some(worktree) = &worktree
        {
            worktree
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend(produces);
        }
        if fail {
            return Err(AppError::new(
                ErrorCode::Internal,
                format!("scripted hook failure for '{reference}'"),
            ));
        }
        Ok(())
    }
}

/// A single bump `on-resolved` invocation recorded by [`RecordingHookRunner`].
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{HookInvocation, HookPhase, HookRunner, RecordingHookRunner};

    #[test]
    fn one_runner_records_before_on_resolved_and_after_through_one_method() {
        // Proves the single HookRunner mechanism spans every lifecycle phase:
        // one runner, one `run_hook`, three phases — no phase-specific trait.
        let runner = RecordingHookRunner::new();
        runner
            .run_hook(HookInvocation::Before, "gate")
            .expect("before hook runs");
        runner
            .run_hook(
                HookInvocation::OnResolved {
                    version_map: Path::new("versions.json"),
                },
                "sync",
            )
            .expect("on-resolved hook runs");
        runner
            .run_hook(HookInvocation::After, "notify")
            .expect("after hook runs");

        let calls = runner.calls();
        let phases: Vec<HookPhase> = calls.iter().map(|call| call.phase).collect();
        assert_eq!(
            phases,
            [HookPhase::Before, HookPhase::OnResolved, HookPhase::After]
        );
        let references: Vec<&str> = calls.iter().map(|call| call.reference.as_str()).collect();
        assert_eq!(references, ["gate", "sync", "notify"]);
        assert_eq!(
            calls[1].version_map.as_deref(),
            Some(Path::new("versions.json"))
        );
    }
}
