//! Outcomes returned by a [`CommandRunner`](super::CommandRunner).
//!
//! A normal invocation produces a [`RunOutcome`] (it ran to completion). A
//! persistent invocation produces a [`StartOutcome`]: either it reached
//! readiness and handed back a [`HeldProcess`] for the engine to hold and later
//! tear down, or it failed readiness.

use rskit_errors::AppResult;
use toven_model::UnitOutput;

/// The result of running a normal (non-persistent) invocation to completion.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct RunOutcome {
    /// Whether the process exited successfully (exit code `0`).
    pub success: bool,
    /// The process exit code, or `None` if it was killed by a signal.
    pub exit_code: Option<i32>,
    /// Raw child output captured during the run, in arrival order.
    ///
    /// The wave walk routes these chunks through the per-unit output channel
    /// (buffered into one labeled block for normal units).
    pub output: Vec<UnitOutput>,
}

impl RunOutcome {
    /// A successful outcome (exit code `0`) carrying `output`.
    #[must_use]
    pub const fn succeeded(output: Vec<UnitOutput>) -> Self {
        Self {
            success: true,
            exit_code: Some(0),
            output,
        }
    }

    /// A failed outcome with `exit_code` carrying `output`.
    #[must_use]
    pub const fn failed(exit_code: Option<i32>, output: Vec<UnitOutput>) -> Self {
        Self {
            success: false,
            exit_code,
            output,
        }
    }
}

/// A running persistent process held in the background after readiness.
///
/// The engine keeps these in a reference-counted held set and tears each down
/// (`shutdown`) when its in-plan dependents drain, or LIFO at run end. Shutdown
/// is synchronous (it mirrors `rskit-process`'s blocking persistent teardown).
pub trait HeldProcess: Send + Sync {
    /// The unit id of the held process (for teardown reporting + LIFO order).
    fn unit_id(&self) -> &str;

    /// Gracefully stop the held process.
    ///
    /// # Errors
    /// Propagates a teardown failure (e.g. the process could not be signalled).
    fn shutdown(self: Box<Self>) -> AppResult<()>;
}

/// The result of starting a persistent invocation.
///
/// Intentionally closed (not `#[non_exhaustive]`): a persistent start is a
/// binary lifecycle outcome — it either reached readiness
/// ([`StartOutcome::Ready`]) or it did not ([`StartOutcome::FailedReadiness`]).
/// There is no third outcome, so callers (the engine work pool) match it
/// exhaustively on purpose.
pub enum StartOutcome {
    /// The process reached readiness; it is now held in the background.
    Ready {
        /// Raw output captured up to the readiness point.
        output: Vec<UnitOutput>,
        /// The held process handle the engine owns until teardown.
        process: Box<dyn HeldProcess>,
    },
    /// The process never reached readiness (timeout or early crash) — a
    /// failure.
    FailedReadiness {
        /// Raw output captured before the readiness failure.
        output: Vec<UnitOutput>,
    },
}

impl StartOutcome {
    /// Whether the persistent unit reached readiness (unblocks dependents).
    #[must_use]
    pub const fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }
}

impl std::fmt::Debug for StartOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ready { output, process } => formatter
                .debug_struct("Ready")
                .field("unit_id", &process.unit_id())
                .field("output_chunks", &output.len())
                .finish(),
            Self::FailedReadiness { output } => formatter
                .debug_struct("FailedReadiness")
                .field("output_chunks", &output.len())
                .finish(),
        }
    }
}
