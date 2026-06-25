//! Config-less scaffolding: detect a Go module and emit a minimal
//! `[ecosystems.go]` fragment for `toven generate`.

use std::path::Path;

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::safe_join;
use toml::{Table, Value};
use toven_model::EcosystemId;
use toven_ports::EcosystemFragment;

/// The manifest filename that marks a Go module root.
const ROOT_MANIFEST: &str = "go.mod";

/// Detect a Go module under `project_root` and, if present, emit the minimal
/// `[ecosystems.go]` fragment (`modules = ["go.mod"]`). Returns `None` when no
/// root `go.mod` exists.
pub(crate) fn scaffold(project_root: &Path) -> AppResult<Option<EcosystemFragment>> {
    let manifest = safe_join(project_root, Path::new(ROOT_MANIFEST)).map_err(|error| {
        AppError::new(ErrorCode::Internal, "failed to resolve go.mod path").with_cause(error)
    })?;
    if !manifest.is_file() {
        return Ok(None);
    }

    let ecosystem = EcosystemId::new("go")?;
    let mut table = Table::new();
    table.insert(
        "modules".to_string(),
        Value::Array(vec![Value::String(ROOT_MANIFEST.to_string())]),
    );
    Ok(Some(EcosystemFragment::new(ecosystem, table)))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use rskit_fs::TempDir;
    use toml::Value;

    use super::scaffold;

    #[test]
    fn absent_manifest_yields_none() {
        let dir = TempDir::new().unwrap();
        assert!(scaffold(dir.path()).unwrap().is_none());
    }

    #[test]
    fn present_manifest_yields_minimal_fragment() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("go.mod"),
            "module example.com/x\n\ngo 1.26\n",
        )
        .unwrap();

        let fragment = scaffold(dir.path()).unwrap().expect("fragment");
        assert_eq!(fragment.ecosystem.as_str(), "go");
        assert_eq!(
            fragment.table.get("modules"),
            Some(&Value::Array(vec![Value::String("go.mod".to_string())]))
        );
    }
}
