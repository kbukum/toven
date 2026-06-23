//! Shared cache-lookup port doubles: [`FakeCacheStore`] (scripted hits) and
//! [`RecordingCacheStore`] (records every queried key, always misses).
//!
//! PLAN cache-verdict tests script which content keys are present, or capture
//! the deterministic content keys a plan produces, without a real backend.

use std::collections::BTreeSet;
use std::sync::Mutex;

use rskit_errors::AppResult;
use toven_ports::CacheStore;

/// A [`CacheStore`] backed by an explicit set of present keys.
///
/// Every key added with [`with_key`](Self::with_key) reports as a hit; all other
/// lookups miss.
#[derive(Debug, Default)]
pub struct FakeCacheStore {
    present: BTreeSet<String>,
}

impl FakeCacheStore {
    /// Construct an empty store where every lookup misses.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark `key` as present so its lookup reports a hit.
    #[must_use]
    pub fn with_key(mut self, key: impl Into<String>) -> Self {
        self.present.insert(key.into());
        self
    }
}

impl CacheStore for FakeCacheStore {
    fn contains(&self, key: &str) -> AppResult<bool> {
        Ok(self.present.contains(key))
    }
}

/// A [`CacheStore`] that records every queried key (and always misses), so a test
/// can capture the deterministic content keys a plan produces.
///
/// Interior mutability ([`Mutex`]) keeps it `&self`-callable and `Send + Sync`
/// behind `dyn CacheStore`. Inspect the recorded keys with
/// [`queried`](Self::queried).
#[derive(Debug, Default)]
pub struct RecordingCacheStore {
    queried: Mutex<Vec<String>>,
}

impl RecordingCacheStore {
    /// Construct an empty recorder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A snapshot of the queried keys, in lookup order.
    #[must_use]
    pub fn queried(&self) -> Vec<String> {
        self.queried
            .lock()
            .expect("RecordingCacheStore mutex poisoned")
            .clone()
    }
}

impl CacheStore for RecordingCacheStore {
    fn contains(&self, key: &str) -> AppResult<bool> {
        self.queried
            .lock()
            .expect("RecordingCacheStore mutex poisoned")
            .push(key.to_string());
        Ok(false)
    }
}
