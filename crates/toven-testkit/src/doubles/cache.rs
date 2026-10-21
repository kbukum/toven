//! Shared cache port doubles: [`FakeCacheStore`] for scripted, recorded lookups
//! and [`RecordingCacheWriter`] for recorded writes.
//!
//! PLAN cache-verdict tests script which content keys are present, or capture
//! the deterministic content keys a plan produces, without a real backend.

use std::collections::BTreeSet;
use std::sync::Mutex;

use rskit_errors::AppResult;
use toven_model::RepoPath;
use toven_ports::{CacheStore, CacheWriter};

/// A [`CacheStore`] backed by an explicit set of present keys.
///
/// Every key added with [`with_key`](Self::with_key) reports as a hit; all
/// other lookups miss. A key added with
/// [`with_stale_outputs`](Self::with_stale_outputs) is present but its outputs
/// no longer match, so its lookup misses too.
#[derive(Debug, Default)]
pub struct FakeCacheStore {
    present: BTreeSet<String>,
    stale: BTreeSet<String>,
    queried: Mutex<Vec<(String, Vec<RepoPath>)>>,
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

    /// Mark `key` as recorded but with outputs that are now missing or
    /// changed, so its lookup misses.
    #[must_use]
    pub fn with_stale_outputs(mut self, key: impl Into<String>) -> Self {
        self.stale.insert(key.into());
        self
    }

    /// A snapshot of the queried keys, in lookup order.
    #[must_use]
    pub fn queried(&self) -> Vec<String> {
        self.queried_with_outputs()
            .into_iter()
            .map(|(key, _)| key)
            .collect()
    }

    /// A snapshot of the queried keys with the outputs each lookup checked.
    #[must_use]
    pub fn queried_with_outputs(&self) -> Vec<(String, Vec<RepoPath>)> {
        self.queried
            .lock()
            .expect("FakeCacheStore mutex poisoned")
            .clone()
    }
}

impl CacheStore for FakeCacheStore {
    fn contains(&self, key: &str, outputs: &[RepoPath]) -> AppResult<bool> {
        self.queried
            .lock()
            .expect("FakeCacheStore mutex poisoned")
            .push((key.to_string(), outputs.to_vec()));
        Ok(self.present.contains(key) && !self.stale.contains(key))
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
    recorded: Mutex<Vec<(String, Vec<RepoPath>)>>,
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
        self.recorded_with_outputs()
            .into_iter()
            .map(|(key, _)| key)
            .collect()
    }

    /// A snapshot of the recorded keys with the outputs each record captured.
    #[must_use]
    pub fn recorded_with_outputs(&self) -> Vec<(String, Vec<RepoPath>)> {
        self.recorded
            .lock()
            .expect("RecordingCacheWriter mutex poisoned")
            .clone()
    }
}

impl CacheWriter for RecordingCacheWriter {
    fn record(&self, key: &str, outputs: &[RepoPath]) -> AppResult<()> {
        self.recorded
            .lock()
            .expect("RecordingCacheWriter mutex poisoned")
            .push((key.to_string(), outputs.to_vec()));
        Ok(())
    }
}
