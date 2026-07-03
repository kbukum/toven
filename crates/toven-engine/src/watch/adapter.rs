//! [`RskitFsWatch`]: the production [`WatchSource`] adapter over rskit-fs.

use std::path::PathBuf;
use std::time::Duration;

use futures::StreamExt as _;
use rskit_errors::AppResult;
use rskit_fs::watch::FsWatcher;
use tokio_util::sync::CancellationToken;
use toven_model::AbsPath;
use toven_ports::{ChangeBatch, ChangeBatchStream, WatchSource};

/// The production filesystem-watch adapter, backed by rskit-fs's [`FsWatcher`].
///
/// Translates the injected [`WatchSource`] port onto the recursive, debounced
/// rskit primitive and maps each rskit `FsChangeBatch` into the port-owned
/// [`ChangeBatch`], so the engine's watch loop never depends on the platform
/// watcher (`notify`) directly.
#[derive(Debug, Default, Clone, Copy)]
pub struct RskitFsWatch;

impl RskitFsWatch {
    /// Construct the adapter.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl WatchSource for RskitFsWatch {
    fn changes(
        &self,
        roots: &[AbsPath],
        debounce: Duration,
        cancel: CancellationToken,
    ) -> AppResult<ChangeBatchStream> {
        let paths: Vec<PathBuf> = roots
            .iter()
            .map(|root| root.as_path().to_path_buf())
            .collect();
        let stream = FsWatcher::new(debounce).watch(&paths, cancel)?;
        let mapped = stream.map(|batch| {
            // rskit's `FsChangeBatch` stores paths in a `BTreeSet`, so iterating
            // yields them already sorted and deduplicated — exactly the
            // `ChangeBatch` contract, no re-sort needed here.
            ChangeBatch::new(batch.paths().iter().cloned().collect())
                .with_rescan(batch.rescan_requested())
        });
        Ok(Box::pin(mapped))
    }
}
