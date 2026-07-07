//! Config-less detection: find a Cargo project and probe its test tooling.

use std::path::Path;

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::safe_join;
use serde::{Deserialize, Serialize};
use toml::Table;
use toven_model::EcosystemId;
use toven_ports::Detection;

/// The manifest filename that marks a Cargo project root.
pub(crate) const ROOT_MANIFEST: &str = "Cargo.toml";

/// The nextest config that marks a workspace configured for `cargo-nextest`.
const NEXTEST_CONFIG: &str = ".config/nextest.toml";

/// The adapter-owned facts a Rust [`Detection`] carries to
/// [`render`](crate::render): the detected manifest and whether the workspace is
/// configured for `cargo-nextest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RustFacts {
    /// The repo-relative root manifest (always `Cargo.toml` today).
    pub(crate) manifest: String,
    /// Whether a `.config/nextest.toml` marks the workspace for `cargo-nextest`.
    pub(crate) nextest: bool,
}

impl RustFacts {
    /// Decode the facts from a [`Detection`]'s opaque table.
    ///
    /// # Errors
    /// Returns an error if the facts table is not the shape this adapter wrote.
    pub(crate) fn from_detection(detection: &Detection) -> AppResult<Self> {
        detection.facts.clone().try_into().map_err(|error| {
            AppError::new(ErrorCode::Internal, "invalid rust detection facts").with_cause(error)
        })
    }

    /// Encode the facts into an opaque [`Table`] for a [`Detection`].
    ///
    /// # Errors
    /// Returns an error only if the facts cannot be encoded as TOML.
    fn to_table(&self) -> AppResult<Table> {
        Table::try_from(self).map_err(|error| {
            AppError::new(ErrorCode::Internal, "encode rust facts").with_cause(error)
        })
    }
}

/// Detect a Cargo project under `project_root` and, if present, return a
/// [`Detection`] carrying the probed facts. Returns `None` when no root
/// `Cargo.toml` exists.
///
/// # Errors
/// Propagates a path-resolution or facts-encoding failure.
pub(crate) fn detect(project_root: &Path) -> AppResult<Option<Detection>> {
    let manifest = safe_join(project_root, Path::new(ROOT_MANIFEST)).map_err(|error| {
        AppError::new(ErrorCode::Internal, "failed to resolve Cargo.toml path").with_cause(error)
    })?;
    if !manifest.is_file() {
        return Ok(None);
    }

    let nextest =
        safe_join(project_root, Path::new(NEXTEST_CONFIG)).is_ok_and(|path| path.is_file());

    let facts = RustFacts {
        manifest: ROOT_MANIFEST.to_string(),
        nextest,
    };
    let ecosystem = EcosystemId::new("rust")?;
    Ok(Some(Detection::new(ecosystem, facts.to_table()?)))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use rskit_fs::TempDir;

    use super::{RustFacts, detect};

    #[test]
    fn absent_manifest_yields_none() {
        let dir = TempDir::new().unwrap();
        assert!(detect(dir.path()).unwrap().is_none());
    }

    #[test]
    fn present_manifest_without_nextest_records_cargo_test() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();

        let detection = detect(dir.path()).unwrap().expect("detection");
        assert_eq!(detection.ecosystem.as_str(), "rust");
        let facts = RustFacts::from_detection(&detection).expect("facts");
        assert_eq!(facts.manifest, "Cargo.toml");
        assert!(!facts.nextest);
    }

    #[test]
    fn nextest_config_is_detected() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        fs::create_dir_all(dir.path().join(".config")).unwrap();
        fs::write(dir.path().join(".config/nextest.toml"), "").unwrap();

        let detection = detect(dir.path()).unwrap().expect("detection");
        let facts = RustFacts::from_detection(&detection).expect("facts");
        assert!(facts.nextest);
    }
}
