//! Typed artifact manifests shared by the release supply-chain projections
//! (`release sbom`, `release depgraphs`): a labeled path to a file the command
//! wrote into its bounded output directory.

use std::path::PathBuf;

/// One artifact a release supply-chain projection produced on disk.
///
/// `label` names what the artifact describes (a workspace or module key);
/// `path` locates the written file inside the command's bounded output
/// directory. The engine never re-reads the file — the manifest is the typed
/// result the reporter renders.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ArtifactManifest {
    /// What the artifact describes (a workspace label or module key).
    pub label: String,
    /// Path to the written artifact inside the bounded output directory.
    pub path: PathBuf,
}

impl ArtifactManifest {
    /// Construct a manifest for `label` at `path`.
    #[must_use]
    pub fn new(label: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            label: label.into(),
            path: path.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::ArtifactManifest;

    #[test]
    fn new_sets_label_and_path() {
        let manifest = ArtifactManifest::new("rust:core", "out/core.dot");
        assert_eq!(manifest.label, "rust:core");
        assert_eq!(manifest.path, PathBuf::from("out/core.dot"));
    }
}
