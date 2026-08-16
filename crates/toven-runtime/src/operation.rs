//! [`UnitOperation`] and [`Completed`] — the per-verb seam the engine drives.

use async_trait::async_trait;
use rskit_errors::AppResult;
use tokio_util::sync::CancellationToken;

/// The outcome of running one unit's operation.
///
/// `failed` is the only thing the engine reads — it decides gating and the
/// summary. The typed `outcome` is the verb's own payload (a version decision, a
/// coverage verdict, a child exit) streamed verbatim to the consumer; a
/// non-failure verdict such as "up-to-date" or "unmeasured" is simply a
/// `succeeded` completion carrying that payload.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Completed<T> {
    /// Whether the operation reported failure (gates dependents).
    pub failed: bool,
    /// The typed per-family outcome payload.
    pub outcome: T,
}

impl<T> Completed<T> {
    /// A successful completion carrying `outcome`.
    #[must_use]
    pub const fn succeeded(outcome: T) -> Self {
        Self {
            failed: false,
            outcome,
        }
    }

    /// A failed completion carrying `outcome` (gates its dependents).
    #[must_use]
    pub const fn failed(outcome: T) -> Self {
        Self {
            failed: true,
            outcome,
        }
    }
}

/// A multi-module verb expressed as a shared GATHER plus a per-unit operation.
///
/// The engine resolves [`gather`](UnitOperation::gather) **exactly once** and
/// hands the resulting `Shared` value to every [`run`](UnitOperation::run) call,
/// so all per-unit work is a pure, total function of the gathered value plus the
/// unit's own id. That is what makes the per-unit phase safe to stream and
/// parallelize.
#[async_trait]
pub trait UnitOperation: Send + Sync + 'static {
    /// The workspace-coupled prerequisites resolved once, up front.
    type Shared: Send + Sync + 'static;
    /// The typed per-unit outcome payload streamed to the consumer.
    type Outcome: Clone + Send + 'static;

    /// Resolve the verb's shared, workspace-coupled facts exactly once.
    ///
    /// An operation with nothing shared returns a unit-like value cheaply; the
    /// engine still calls this a single time regardless of unit count.
    ///
    /// # Errors
    /// Propagates any failure resolving the shared prerequisites.
    async fn gather(&self) -> AppResult<Self::Shared>;

    /// Run one unit against the gathered shared value.
    ///
    /// `cancel` fires when the run is aborting (fail-fast or external cancel);
    /// long operations should honour it. Returning `Err` is a hard engine error
    /// (aborts the run); an ordinary unit failure is `Ok(Completed::failed(..))`.
    ///
    /// # Errors
    /// Propagates a hard, non-recoverable operation error.
    async fn run(
        &self,
        shared: &Self::Shared,
        unit_id: &str,
        cancel: CancellationToken,
    ) -> AppResult<Completed<Self::Outcome>>;
}

#[cfg(test)]
mod tests {
    use super::Completed;

    #[test]
    fn constructors_set_the_failed_flag() {
        assert!(!Completed::succeeded(7).failed);
        assert!(Completed::failed(7).failed);
        assert_eq!(Completed::succeeded("v1").outcome, "v1");
    }
}
