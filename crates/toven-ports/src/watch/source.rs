//! The [`WatchSource`] port and its [`ChangeBatchStream`] output type.

use std::pin::Pin;
use std::time::Duration;

use futures::Stream;
use rskit_errors::AppResult;
use tokio_util::sync::CancellationToken;
use toven_model::AbsPath;

use super::ChangeBatch;

/// A cancellable stream of debounced [`ChangeBatch`]es.
///
/// The stream terminates when the watch is cancelled, when every watched root
/// stops emitting, or when the consumer drops it.
pub type ChangeBatchStream = Pin<Box<dyn Stream<Item = ChangeBatch> + Send>>;

/// Observes a set of filesystem roots and yields debounced batches of changes.
///
/// Implementations coalesce raw OS events over a trailing-edge `debounce`
/// window so a burst of saves surfaces as a single batch. The engine's watch
/// loop injects this port to rerun the affected subgraph on each batch; the
/// production adapter wraps rskit-fs's `FsWatcher`, and the testkit double
/// replays scripted batches.
pub trait WatchSource {
    /// Begin watching `roots`, coalescing events over `debounce`.
    ///
    /// # Errors
    ///
    /// Returns an error if a root cannot be watched (e.g. it does not exist or
    /// the platform watcher cannot be initialized).
    fn changes(
        &self,
        roots: &[AbsPath],
        debounce: Duration,
        cancel: CancellationToken,
    ) -> AppResult<ChangeBatchStream>;
}
