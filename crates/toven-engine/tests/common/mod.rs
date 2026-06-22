#![allow(dead_code, clippy::redundant_pub_crate)]
//! Shared helpers for the `toven-engine` config integration tests.

use std::collections::BTreeSet;

use toven_engine::config::CanonicalRegistry;
use toven_model::EcosystemId;

/// Construct a validated [`EcosystemId`] for a test id.
pub(crate) fn eid(id: &str) -> EcosystemId {
    EcosystemId::new(id).expect("test ecosystem id is valid")
}

/// Build the loaded-provider id set from string ids.
pub(crate) fn loaded(ids: &[&str]) -> BTreeSet<EcosystemId> {
    ids.iter().map(|id| eid(id)).collect()
}

/// The canonical registry embedded in `toven-model`.
pub(crate) fn canonical() -> CanonicalRegistry {
    CanonicalRegistry::model()
}

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use rskit_errors::AppResult;
use toven_engine::plan::{CacheStore, SourceDigest, ToolchainProber};
use toven_model::Module;
use toven_ports::ToolchainProbe;

/// A deterministic [`SourceDigest`]: the module/path identity is its own name, so
/// tests control hashes without touching the filesystem.
#[derive(Debug, Default)]
pub(crate) struct StubDigest;

impl SourceDigest for StubDigest {
    fn module(&self, module: &Module) -> AppResult<String> {
        Ok(format!("module:{}", module.id))
    }

    fn path(&self, repo_relative: &Path) -> AppResult<String> {
        Ok(format!("path:{}", repo_relative.display()))
    }
}

/// A [`ToolchainProber`] that counts invocations and returns a fixed version.
#[derive(Debug, Default)]
pub(crate) struct CountingProber {
    calls: AtomicUsize,
}

impl CountingProber {
    pub(crate) fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl ToolchainProber for CountingProber {
    fn probe(&self, _probe: &ToolchainProbe, _workspace_root: &Path) -> AppResult<String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok("v1".to_string())
    }
}

/// A [`CacheStore`] backed by an explicit set of present keys.
#[derive(Debug, Default)]
pub(crate) struct SetCache {
    present: std::collections::BTreeSet<String>,
}

impl SetCache {
    pub(crate) fn with_key(mut self, key: impl Into<String>) -> Self {
        self.present.insert(key.into());
        self
    }
}

impl CacheStore for SetCache {
    fn contains(&self, key: &str) -> AppResult<bool> {
        Ok(self.present.contains(key))
    }
}

/// A [`CacheStore`] that records every queried key (and always misses), so a test
/// can capture the deterministic content keys a plan produces.
#[derive(Debug, Default)]
pub(crate) struct RecordingCache {
    queried: std::sync::Mutex<Vec<String>>,
}

impl RecordingCache {
    pub(crate) fn queried(&self) -> Vec<String> {
        self.queried.lock().expect("cache mutex").clone()
    }
}

impl CacheStore for RecordingCache {
    fn contains(&self, key: &str) -> AppResult<bool> {
        self.queried
            .lock()
            .expect("cache mutex")
            .push(key.to_string());
        Ok(false)
    }
}
