//! Shared cache port doubles: [`FakeCacheStore`] for scripted, recorded lookups
//! and [`RecordingCacheWriter`] for recorded writes.
//!
//! PLAN cache-verdict tests script which content keys are present, or capture
//! the deterministic content keys a plan produces, without a real backend.

use std::collections::BTreeSet;
use std::sync::Mutex;

use rskit_errors::AppResult;
use toven_ports::{CacheStore, CacheWriter};

/// A [`CacheStore`] backed by an explicit set of present keys.
///
/// Every key added with [`with_key`](Self::with_key) reports as a hit; all
/// other lookups miss.
#[derive(Debug, Default)]
pub struct FakeCacheStore {
    present: BTreeSet<String>,
    queried: Mutex<Vec<String>>,
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

    /// A snapshot of the queried keys, in lookup order.
    #[must_use]
    pub fn queried(&self) -> Vec<String> {
        self.queried
            .lock()
            .expect("FakeCacheStore mutex poisoned")
            .clone()
    }
}

impl CacheStore for FakeCacheStore {
    fn contains(&self, key: &str) -> AppResult<bool> {
        self.queried
            .lock()
            .expect("FakeCacheStore mutex poisoned")
            .push(key.to_string());
        Ok(self.present.contains(key))
    }
}

/// A [`CacheWriter`] that records every recorded key (in order) so APPLY tests
/// can assert which successful units wrote a cache record.
///
/// Interior mutability ([`Mutex`]) keeps it `&self`-callable and `Send + Sync`
/// behind `dyn CacheWriter`. Inspect the recorded keys with
/// [`recorded`](Self::recorded).
#[derive(Debug, Default)]
pub struct RecordingCacheWriter {
    recorded: Mutex<Vec<String>>,
}

impl RecordingCacheWriter {
    /// Construct an empty recorder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A snapshot of the recorded keys, in write order.
    #[must_use]
    pub fn recorded(&self) -> Vec<String> {
        self.recorded
            .lock()
            .expect("RecordingCacheWriter mutex poisoned")
            .clone()
    }
}

impl CacheWriter for RecordingCacheWriter {
    fn record(&self, key: &str) -> AppResult<()> {
        self.recorded
            .lock()
            .expect("RecordingCacheWriter mutex poisoned")
            .push(key.to_string());
        Ok(())
    }
}
