//! Shared watch port double: [`ScriptedWatchSource`] replays scripted change
//! batches so the engine's watch loop can be exercised deterministically without
//! a real filesystem watcher.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt as _;
use rskit_errors::AppResult;
use tokio_util::sync::CancellationToken;
use toven_model::AbsPath;
use toven_ports::{ChangeBatch, ChangeBatchStream, WatchSource};

/// A [`WatchSource`] that replays a fixed script of change batches.
///
/// Each scripted entry becomes one [`ChangeBatch`] the loop consumes as a
/// separate rerun trigger. Use [`new`](ScriptedWatchSource::new) for plain path
/// batches or [`from_batches`](ScriptedWatchSource::from_batches) to script
/// batches that carry a rescan signal. By default the stream ends after the last
/// scripted batch (modelling a torn-down watcher, so the loop exits on its own);
/// enable [`stay_open`](ScriptedWatchSource::stay_open) to instead keep the
/// stream pending after the script, so only cancellation ends the loop.
#[derive(Debug, Clone, Default)]
pub struct ScriptedWatchSource {
    batches: Vec<ChangeBatch>,
    stay_open: bool,
    calls: Arc<Mutex<Vec<WatchCall>>>,
}

/// One recorded [`WatchSource::changes`] invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchCall {
    /// The absolute roots the loop asked to watch.
    pub roots: Vec<PathBuf>,
    /// The debounce window the loop requested.
    pub debounce: Duration,
}

impl ScriptedWatchSource {
    /// Construct a source that yields each entry of `batches` as one change
    /// batch of paths (no rescan signal).
    #[must_use]
    pub fn new(batches: Vec<Vec<PathBuf>>) -> Self {
        Self::from_batches(batches.into_iter().map(ChangeBatch::new).collect())
    }

    /// Construct a source that replays the given [`ChangeBatch`]es verbatim,
    /// preserving any rescan signal — use this to script overflow/rescan
    /// batches.
    #[must_use]
    pub fn from_batches(batches: Vec<ChangeBatch>) -> Self {
        Self {
            batches,
            stay_open: false,
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Keep the stream pending after the scripted batches instead of ending it,
    /// so the loop exits only when its cancellation token fires.
    #[must_use]
    pub const fn stay_open(mut self) -> Self {
        self.stay_open = true;
        self
    }

    /// The recorded [`WatchSource::changes`] calls, for assertions.
    #[must_use]
    pub fn calls(&self) -> Vec<WatchCall> {
        self.calls.lock().expect("watch calls lock").clone()
    }
}

impl WatchSource for ScriptedWatchSource {
    fn changes(
        &self,
        roots: &[AbsPath],
        debounce: Duration,
        cancel: CancellationToken,
    ) -> AppResult<ChangeBatchStream> {
        self.calls
            .lock()
            .expect("watch calls lock")
            .push(WatchCall {
                roots: roots
                    .iter()
                    .map(|root| root.as_path().to_path_buf())
                    .collect(),
                debounce,
            });

        let batches = self.batches.clone();
        let scripted = futures::stream::iter(batches);
        if self.stay_open {
            // After the script, block on cancellation so the loop is driven to
            // exit by its token rather than by a stream end.
            let tail = futures::stream::once(async move {
                cancel.cancelled().await;
                None
            })
            .filter_map(|value| async move { value });
            Ok(Box::pin(scripted.chain(tail)))
        } else {
            Ok(Box::pin(scripted))
        }
    }
}
