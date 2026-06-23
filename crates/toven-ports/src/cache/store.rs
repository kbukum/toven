//! [`CacheStore`] — the injected cache-record lookup port.

use rskit_errors::AppResult;

/// A read-only view over existing cache records, keyed by content hash.
///
/// PLAN only *reads* whether a usable record exists for a content key; writing
/// records is an APPLY concern. The port is injected so the planner stays pure
/// and tests substitute a deterministic store, while the concrete backend lives
/// in the engine.
pub trait CacheStore {
    /// Whether a reusable record exists for `key`.
    ///
    /// # Errors
    /// Propagates a backing-store read failure.
    fn contains(&self, key: &str) -> AppResult<bool>;
}
