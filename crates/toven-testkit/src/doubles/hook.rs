//! Shared [`HookRunner`] double: [`RecordingHookRunner`].
//!
//! Verb tests (release first) inject this instead of the real PLAN→APPLY hook
//! runner: it records every `(phase, reference)` it is asked to run so a test
//! can assert ordering and fail-closed semantics (a failing `pre` hook must
//! abort before any mutation), and it can be scripted to fail on a chosen
//! reference. It is `Clone` (shared state) so a test can hold a handle for
//! assertions after injecting it.

use std::sync::{Arc, Mutex};

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_ports::{HookPhase, HookRunner};

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
    fn run_hook(&self, phase: HookPhase, reference: &str) -> AppResult<()> {
        // Record the attempt before failing so a test can assert a failing `pre`
        // hook was reached (and that nothing after it ran).
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
