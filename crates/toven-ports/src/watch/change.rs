//! The [`ChangeBatch`] value type: one debounce window's changed paths.

use std::path::PathBuf;

/// A debounced batch of absolute filesystem paths that changed together.
///
/// Paths are deduplicated and sorted by the watch adapter, so a batch is
/// deterministic regardless of the order the underlying OS events arrived. The
/// engine relativizes them against the workspace root before mapping to modules.
///
/// A batch may additionally carry a **rescan** signal
/// ([`rescan_requested`](Self::rescan_requested)): the platform watcher dropped
/// events during the window (typically a queue overflow), so [`paths`](Self::paths)
/// may be incomplete. The watch loop treats a rescan as "re-evaluate the whole
/// watched scope" rather than trusting the partial path list. A rescan-only batch
/// has empty [`paths`](Self::paths) but is **not** [`is_empty`](Self::is_empty).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangeBatch {
    paths: Vec<PathBuf>,
    rescan: bool,
}

impl ChangeBatch {
    /// Build a batch from an already-sorted, deduplicated set of changed paths,
    /// with no rescan signal.
    #[must_use]
    pub const fn new(paths: Vec<PathBuf>) -> Self {
        Self {
            paths,
            rescan: false,
        }
    }

    /// Set whether this batch requests a rescan (the watcher dropped events, so
    /// [`paths`](Self::paths) may be incomplete).
    #[must_use]
    pub const fn with_rescan(mut self, rescan: bool) -> Self {
        self.rescan = rescan;
        self
    }

    /// The changed paths in this batch.
    ///
    /// May be empty even when the batch is meaningful — see
    /// [`rescan_requested`](Self::rescan_requested).
    #[must_use]
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// Whether the platform watcher dropped events during this window, so the
    /// reported [`paths`](Self::paths) may be incomplete and the watch loop
    /// should re-evaluate the whole watched scope from scratch.
    #[must_use]
    pub const fn rescan_requested(&self) -> bool {
        self.rescan
    }

    /// Whether the batch carries no information — no changed paths and no rescan
    /// signal.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.paths.is_empty() && !self.rescan
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::ChangeBatch;

    #[test]
    fn batch_exposes_its_paths() {
        let batch = ChangeBatch::new(vec![
            PathBuf::from("/repo/a.rs"),
            PathBuf::from("/repo/b.rs"),
        ]);
        assert_eq!(batch.paths().len(), 2);
        assert!(!batch.is_empty());
        assert!(batch.paths().iter().any(|path| path.ends_with("a.rs")));
    }

    #[test]
    fn default_batch_is_empty() {
        let batch = ChangeBatch::default();
        assert!(batch.is_empty());
        assert!(batch.paths().is_empty());
        assert!(!batch.rescan_requested());
    }

    #[test]
    fn rescan_only_batch_is_not_empty() {
        let batch = ChangeBatch::new(Vec::new()).with_rescan(true);
        assert!(batch.rescan_requested());
        assert!(batch.paths().is_empty());
        assert!(!batch.is_empty(), "a rescan signal must not read as empty");
    }

    #[test]
    fn new_defaults_rescan_off() {
        let batch = ChangeBatch::new(vec![PathBuf::from("/repo/a.rs")]);
        assert!(!batch.rescan_requested());
    }
}
