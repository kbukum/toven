//! [`ToolchainProber`] — the injected toolchain-probe port.

use std::path::Path;

use rskit_errors::AppResult;

use crate::task::ToolchainProbe;

/// The injected toolchain probe: run a probe command and return its version
/// line.
///
/// Implementations execute the probe (a side effect) and return the opaque,
/// cache-significant version string; the engine folds it into the cache key.
/// The port keeps the planner pure so tests substitute a deterministic prober,
/// and the concrete subprocess adapter lives in the engine.
pub trait ToolchainProber {
    /// Probe the toolchain in `workspace_root`, returning the version identity.
    ///
    /// # Errors
    /// A probe that cannot run or exits unsuccessfully is a hard error.
    fn probe(&self, probe: &ToolchainProbe, workspace_root: &Path) -> AppResult<String>;
}
