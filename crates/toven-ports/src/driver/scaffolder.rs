//! [`DriverScaffolder`] — probe an out-of-process driver for scaffold fragments.

use std::path::Path;

use rskit_errors::AppResult;

use crate::EcosystemFragment;

/// Probes an out-of-process `toven-<eco>` driver for the fragments it detects.
///
/// Injected so generation stays testable without spawning a real subprocess; the
/// engine's production adapter drives the federated `__scaffold` exchange.
pub trait DriverScaffolder {
    /// Ask the driver at `program` what ecosystems it detects under `project_root`.
    ///
    /// # Errors
    /// Returns a typed error if the driver cannot be reached or reports a
    /// scaffold failure. A *located* driver that misbehaves is a hard error.
    fn scaffold(&self, program: &Path, project_root: &Path) -> AppResult<Vec<EcosystemFragment>>;
}
