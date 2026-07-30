//! Version-probe port: run a packaged binary and read the version it reports.
//!
//! The *expected* version and the pass/fail decision are release-engine domain;
//! this port is the thin reusable sliver: execute this binary and return the
//! single version line it prints (argv-only, no shell). The engine compares the
//! reported line against the version it decided, so a mismatched or corrupted
//! archive fails closed.

use std::path::Path;

use rskit_errors::AppResult;

/// Runs a packaged binary and returns the version string it reports.
pub trait VersionProbe: Send + Sync {
    /// Execute `binary` (e.g. `binary --version`) and return the trimmed line it
    /// prints on standard output.
    ///
    /// # Errors
    /// Fails closed when the binary cannot be executed or produces no readable
    /// version output.
    fn report_version(&self, binary: &Path) -> AppResult<String>;
}
