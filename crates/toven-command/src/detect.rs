//! Config-less detection for the command adapter.

use std::path::Path;

use toven_ports::Detection;

/// Detect a command ecosystem under `project_root`.
///
/// The command adapter is purely opt-in and has no canonical manifest or file
/// convention, so it never self-detects.
pub(crate) const fn detect(_project_root: &Path) -> Option<Detection> {
    None
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::detect;

    #[test]
    fn command_never_self_detects() {
        assert!(detect(Path::new(".")).is_none());
    }
}
