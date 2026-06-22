//! The cache-record lookup port read during the Cache-decision phase.
//!
//! PLAN only *reads* whether a usable record exists for a content key; writing
//! records is an APPLY concern (step 8). The port is injected so the planner
//! stays pure and tests substitute a deterministic store.

use rskit_errors::AppResult;

/// A read-only view over existing cache records, keyed by content hash.
pub trait CacheStore {
    /// Whether a reusable record exists for `key`.
    ///
    /// # Errors
    /// Propagates a backing-store read failure.
    fn contains(&self, key: &str) -> AppResult<bool>;
}

/// A [`CacheStore`] with no records: every lookup is a miss.
///
/// The default when no cache backend is wired (e.g. `--explain`); every
/// [`ReadWrite`](super::super::request::CacheMode::ReadWrite) unit becomes a
/// [`Miss`](toven_model::CacheVerdict::Miss).
#[derive(Debug, Clone, Copy, Default)]
pub struct NullCache;

impl CacheStore for NullCache {
    fn contains(&self, _key: &str) -> AppResult<bool> {
        Ok(false)
    }
}
