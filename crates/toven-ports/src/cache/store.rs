//! [`CacheStore`] — the injected cache-record lookup port.

use rskit_errors::AppResult;
use toven_model::RepoPath;

/// A read-only view over existing cache records, keyed by content hash.
///
/// PLAN only *reads* whether a usable record exists for a content key; writing
/// records is an APPLY concern. The port is injected so the planner stays pure
/// and tests substitute a deterministic store, while the concrete backend lives
/// in the engine.
pub trait CacheStore: Send + Sync {
    /// Whether a reusable record exists for `key` and every declared output
    /// it recorded is still present and unchanged.
    ///
    /// `outputs` are the unit's declared outputs (repo-relative). A record
    /// whose outputs are missing or differ from what the run produced is not
    /// reusable, because the cache skips a unit without restoring files.
    ///
    /// # Errors
    /// Propagates a backing-store or output read failure.
    fn contains(&self, key: &str, outputs: &[RepoPath]) -> AppResult<bool>;
}
