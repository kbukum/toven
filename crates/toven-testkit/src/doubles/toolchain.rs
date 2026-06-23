//! Shared toolchain-probe port double: [`CountingToolchainProber`].
//!
//! Toolchain-resolution tests substitute this deterministic prober instead of
//! spawning a real subprocess, and assert how many times `probe` was invoked
//! via [`CountingToolchainProber::calls`] (a total call count, not per-workspace).

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use rskit_errors::AppResult;
use toven_ports::{ToolchainProbe, ToolchainProber};

/// A [`ToolchainProber`] that counts invocations and returns a fixed version.
///
/// Interior mutability ([`AtomicUsize`]) keeps it `&self`-callable and
/// `Send + Sync` behind `dyn ToolchainProber`. Inspect the probe count with
/// [`calls`](Self::calls).
#[derive(Debug, Default)]
pub struct CountingToolchainProber {
    calls: AtomicUsize,
    version: Option<String>,
}

impl CountingToolchainProber {
    /// Construct a prober that returns the default `"v1"` version.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Script the version string every probe returns.
    #[must_use]
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    /// The number of probe invocations recorded so far.
    #[must_use]
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl ToolchainProber for CountingToolchainProber {
    fn probe(&self, _probe: &ToolchainProbe, _workspace_root: &Path) -> AppResult<String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.version.clone().unwrap_or_else(|| "v1".to_string()))
    }
}
