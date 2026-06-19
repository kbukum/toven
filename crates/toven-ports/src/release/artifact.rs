//! The built, publishable artifact a [`ReleaseTarget`](super::ReleaseTarget) produces.

use std::path::PathBuf;

use toven_model::Metadata;

/// A packaged artifact ready to publish.
///
/// Intentionally thin: the engine never inspects the artifact's format, it only
/// hands it back to [`ReleaseTarget::publish`](super::ReleaseTarget::publish).
/// `path` locates the built artifact (e.g. a `.crate` file); `metadata` carries
/// any ecosystem-specific facts the adapter needs on the publish round-trip.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Artifact {
    /// Location of the packaged artifact on disk.
    pub path: PathBuf,
    /// Freeform adapter data carried from `package` to `publish`.
    pub metadata: Metadata,
}

impl Artifact {
    /// Construct an artifact at `path` with empty metadata.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            metadata: Metadata::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::Artifact;

    #[test]
    fn new_sets_path_and_empty_metadata() {
        let artifact = Artifact::new("dist/pkg.crate");
        assert_eq!(artifact.path, PathBuf::from("dist/pkg.crate"));
        assert!(artifact.metadata.is_empty());
    }
}
