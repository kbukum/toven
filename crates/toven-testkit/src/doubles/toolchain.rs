//! Shared toolchain-probe port double: [`ScriptedToolchainProber`].
//!
//! Toolchain-resolution tests substitute this deterministic prober instead of
//! spawning a real subprocess, and assert how many times `probe` was invoked
//! via [`ScriptedToolchainProber::calls`] (a total call count, not
//! per-workspace).

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_ports::{ToolchainProbe, ToolchainProber};

/// A [`ToolchainProber`] that classifies a probe by its program name.
///
/// Deterministic substitute for the process prober in audit/`doctor` tests: a
/// program registered as absent via [`with_absent`](Self::with_absent) yields
/// the same typed `NotFound` error the real prober maps a spawn `ENOENT` to
/// (so "missing tool" is exercised without an unresolvable binary on the host);
/// every other program is present and returns the scripted version. Interior
/// mutability keeps it `&self`-callable behind `dyn ToolchainProber`.
#[derive(Debug, Default)]
pub struct ScriptedToolchainProber {
    calls: AtomicUsize,
    absent: BTreeSet<String>,
    version: Option<String>,
}

impl ScriptedToolchainProber {
    /// Construct a prober where every program is present with the default
    /// `"v1"` version.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `program` as absent: probing it fails with a typed `NotFound`,
    /// exactly as the process prober reports a tool missing from `PATH`.
    #[must_use]
    pub fn with_absent(mut self, program: impl Into<String>) -> Self {
        self.absent.insert(program.into());
        self
    }

    /// Script the version string a present program's probe returns.
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

impl ToolchainProber for ScriptedToolchainProber {
    fn probe(&self, probe: &ToolchainProbe, _workspace_root: &Path) -> AppResult<String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.absent.contains(&probe.program) {
            return Err(AppError::new(
                ErrorCode::NotFound,
                format!("'{}' is not installed or on PATH", probe.program),
            ));
        }
        Ok(self.version.clone().unwrap_or_else(|| "v1".to_string()))
    }
}
