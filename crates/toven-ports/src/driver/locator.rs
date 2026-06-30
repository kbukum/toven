//! [`DriverLocator`] — resolve a conventional driver binary name to a path.

use std::path::PathBuf;

use rskit_errors::AppResult;

/// Locates a driver binary by its conventional name (e.g. `toven-go`).
///
/// Injected so resolution stays pure and testable without touching the real
/// `PATH`.
pub trait DriverLocator {
    /// Resolve `binary_name` to an executable path, or `None` if not found.
    ///
    /// # Errors
    /// Returns a typed error if a candidate cannot be inspected (e.g. a
    /// filesystem error while checking executability), so an errored check is
    /// never silently treated as "absent".
    fn locate(&self, binary_name: &str) -> AppResult<Option<PathBuf>>;
}
