//! [`CacheWriter`] — the injected cache-record write port.

use rskit_errors::AppResult;
use toven_model::RepoPath;

/// Writes cache records keyed by content hash on successful execution.
///
/// The PLAN-side [`CacheStore`](super::CacheStore) is read-only by design (it
/// only decides HIT/MISS); recording a fresh record is an APPLY concern. The
/// write half is a separate port so the planner stays read-only and APPLY
/// injects the writer, with the concrete backend living in the engine.
pub trait CacheWriter: Send + Sync {
    /// Record a reusable cache entry for `key` after a unit succeeds,
    /// capturing the state of its declared `outputs` (repo-relative).
    ///
    /// A run that leaves a declared output missing is not recorded, so the
    /// next run executes it again.
    ///
    /// # Errors
    /// Propagates a backing-store write or output read failure.
    fn record(&self, key: &str, outputs: &[RepoPath]) -> AppResult<()>;
}
