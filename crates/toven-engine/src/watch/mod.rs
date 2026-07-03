//! `watch` — the watch-mode PLAN→APPLY loop and its filesystem-watch adapter.
//!
//! [`WatchSession`] owns the rerun loop over the injected
//! [`WatchSource`](toven_ports::WatchSource) port; [`RskitFsWatch`] is the
//! production adapter binding that port to rskit-fs's recursive, debounced
//! `FsWatcher`. The testkit double supplies scripted batches for deterministic
//! tests.

mod adapter;
mod run;

pub use adapter::RskitFsWatch;
pub use run::{WatchSession, watch_roots};
