//! [`CommandRunner`] — the injected process-execution port.

use async_trait::async_trait;
use rskit_errors::AppResult;
use tokio_util::sync::CancellationToken;

use super::{Invocation, OutputObserver, RunOutcome, StartOutcome};

/// Executes resolved [`Invocation`]s, cancellably and with bounded output.
///
/// This is the APPLY seam mirroring the PLAN ports
/// ([`VcsReader`](crate::VcsReader), [`ToolchainProber`](crate::ToolchainProber),
/// …): the wave walk drives a `&dyn CommandRunner` so it is unit-tested against a
/// scriptable runner with no real subprocess, while the concrete
/// `rskit-process`-backed runner lives in the engine. Methods take a
/// [`CancellationToken`] so `--fail-fast` (and persistent teardown) can stop
/// in-flight work.
#[async_trait]
pub trait CommandRunner: Send + Sync {
    /// Run a normal invocation to completion.
    ///
    /// # Errors
    /// Propagates a spawn/IO failure. A non-zero exit is *not* an error: it is a
    /// [`RunOutcome`] with `success = false` so the wave walk can gate on it.
    async fn run(
        &self,
        invocation: &Invocation,
        cancel: CancellationToken,
    ) -> AppResult<RunOutcome>;

    /// Start a persistent invocation and wait for its readiness policy.
    ///
    /// Returns once the process is ready (handing back a held handle) or once
    /// readiness fails. The held process stays alive in the background until the
    /// engine tears it down.
    ///
    /// # Errors
    /// Propagates a spawn/IO failure. A readiness timeout/early crash is *not* an
    /// error: it is [`StartOutcome::FailedReadiness`] so gating fails closed.
    async fn start_persistent(
        &self,
        invocation: &Invocation,
        cancel: CancellationToken,
        output: OutputObserver,
    ) -> AppResult<StartOutcome>;
}
