//! The filesystem-watch port: a debounced source of changed-path batches.
//!
//! [`WatchSource`] is the seam the engine's watch loop injects to observe the
//! workspace tree and rerun the affected subgraph on each change. The concrete
//! adapter (`RskitFsWatch`, over rskit-fs's `FsWatcher`) lives in the engine;
//! the `toven-testkit` double feeds scripted [`ChangeBatch`]es for
//! deterministic tests. The port speaks Toven's own path vocabulary so this
//! layer never links the platform watcher (`notify`).

mod change;
mod source;

pub use change::ChangeBatch;
pub use source::{ChangeBatchStream, WatchSource};
