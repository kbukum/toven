//! The built, publishable artifact a [`ReleaseTarget`](super::ReleaseTarget) produces.

use std::path::PathBuf;

/// A packaged artifact ready to publish.
///
/// Intentionally thin: the engine never inspects the artifact's format, it only
/// hands it back to [`ReleaseTarget::publish`](super::ReleaseTarget::publish).
/// `path` locates the built artifact (e.g. a `.crate` file).
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Artifact {
    /// Location of the packaged artifact on disk.
    pub path: PathBuf,
}

impl Artifact {
    /// Construct an artifact at `path`.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::Artifact;

    #[test]
    fn new_sets_path() {
        let artifact = Artifact::new("dist/pkg.crate");
        assert_eq!(artifact.path, PathBuf::from("dist/pkg.crate"));
    }
}
