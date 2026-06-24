//! [`CacheWriter`] — the injected cache-record write port.

use rskit_errors::AppResult;

/// Writes cache records keyed by content hash on successful execution.
///
/// The PLAN-side [`CacheStore`](super::CacheStore) is read-only by design (it
/// only decides HIT/MISS); recording a fresh record is an APPLY concern. The
/// write half is a separate port so the planner stays read-only and APPLY
/// injects the writer, with the concrete backend living in the engine.
pub trait CacheWriter: Send + Sync {
    /// Record a reusable cache entry for `key` after a unit succeeds.
    ///
    /// # Errors
    /// Propagates a backing-store write failure.
    fn record(&self, key: &str) -> AppResult<()>;
}
