//! Config-less detection: find a root Go module and carry its wizard facts.

use std::path::Path;

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::safe_join;
use serde::{Deserialize, Serialize};
use toml::Table;
use toven_model::EcosystemId;
use toven_ports::Detection;

/// The manifest filename that marks a Go module root.
pub(crate) const ROOT_MANIFEST: &str = "go.mod";

/// The adapter-owned facts a Go [`Detection`] carries to [`render`](crate::render).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct GoFacts {
    /// The repo-relative root manifest (always `go.mod` today).
    pub(crate) manifest: String,
}

impl GoFacts {
    /// Decode the facts from a [`Detection`]'s opaque table.
    ///
    /// # Errors
    /// Returns an error if the facts table is not the shape this adapter wrote.
    pub(crate) fn from_detection(detection: &Detection) -> AppResult<Self> {
        detection.facts.clone().try_into().map_err(|error| {
            AppError::new(ErrorCode::Internal, "invalid go detection facts").with_cause(error)
        })
    }

    /// Encode the facts into an opaque [`Table`] for a [`Detection`].
    ///
    /// # Errors
    /// Returns an error only if the facts cannot be encoded as TOML.
    fn to_table(&self) -> AppResult<Table> {
        Table::try_from(self).map_err(|error| {
            AppError::new(ErrorCode::Internal, "encode go facts").with_cause(error)
        })
    }
}

/// Detect a Go module under `project_root` and, if present, return a
/// [`Detection`] carrying the probed facts. Returns `None` when no root `go.mod`
/// exists.
///
/// # Errors
/// Propagates a path-resolution or facts-encoding failure.
pub(crate) fn detect(project_root: &Path) -> AppResult<Option<Detection>> {
    let manifest = safe_join(project_root, Path::new(ROOT_MANIFEST)).map_err(|error| {
        AppError::new(ErrorCode::Internal, "failed to resolve go.mod path").with_cause(error)
    })?;
    if !manifest.is_file() {
        return Ok(None);
    }

    let facts = GoFacts {
        manifest: ROOT_MANIFEST.to_string(),
    };
    let ecosystem = EcosystemId::new("go")?;
    Ok(Some(Detection::new(ecosystem, facts.to_table()?)))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use rskit_fs::TempDir;

    use super::{GoFacts, detect};

    #[test]
    fn absent_manifest_yields_none() {
        let dir = TempDir::new().unwrap();
        assert!(detect(dir.path()).unwrap().is_none());
    }

    #[test]
    fn present_manifest_records_go_mod() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("go.mod"),
            "module example.com/x\n\ngo 1.26\n",
        )
        .unwrap();

        let detection = detect(dir.path()).unwrap().expect("detection");
        assert_eq!(detection.ecosystem.as_str(), "go");
        let facts = GoFacts::from_detection(&detection).expect("facts");
        assert_eq!(facts.manifest, "go.mod");
    }
}
