//! The concrete no-op [`CacheStore`] used when no cache backend is wired.
//!
//! PLAN only *reads* whether a usable record exists for a content key; writing
//! records is an APPLY concern. The [`CacheStore`] port is injected so the
//! planner stays pure and tests substitute a deterministic store.

use rskit_errors::AppResult;
use toven_ports::CacheStore;

/// A [`CacheStore`] with no records: every lookup is a miss.
///
/// The default when no cache backend is wired (e.g. `--explain`); every
/// `ReadWrite` cache-mode unit becomes a
/// [`Miss`](toven_model::CacheVerdict::Miss).
#[derive(Debug, Clone, Copy, Default)]
pub struct NullCache;

impl CacheStore for NullCache {
    fn contains(&self, _key: &str) -> AppResult<bool> {
        Ok(false)
    }
}
