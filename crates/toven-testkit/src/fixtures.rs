//! Locate and load fixtures from this crate's shared `fixtures/` tree.
//!
//! Every loader resolves relative to **this crate's** `CARGO_MANIFEST_DIR`, so a
//! fixture added here is reachable from any consumer crate's tests regardless of
//! where that crate lives. Paths are joined through `rskit-fs` `safe_join`, so a
//! traversing or absolute relative path is rejected, and a missing fixture
//! surfaces as a clear [`AppError`] rather than a silent skip.

use std::path::{Path, PathBuf};

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::{safe_join, sync_io::dir, sync_io::file};

/// The shared fixture root, captured at this crate's compile time.
pub const FIXTURES_ROOT: &str = env!("CARGO_MANIFEST_DIR");

/// Absolute path to the shared `fixtures/` directory.
#[must_use]
pub fn root() -> PathBuf {
    Path::new(FIXTURES_ROOT).join("fixtures")
}

/// Resolve a safe path under the shared fixtures root.
///
/// Rejects absolute or parent-traversing relative paths.
pub fn path(rel: impl AsRef<Path>) -> AppResult<PathBuf> {
    safe_join(&root(), rel.as_ref())
        .map_err(|error| AppError::invalid_input("fixture_path", error.to_string()))
}

/// Resolve a fixture path and confirm it exists, with a clear error otherwise.
fn existing(rel: impl AsRef<Path>) -> AppResult<PathBuf> {
    let resolved = path(rel.as_ref())?;
    if resolved.exists() {
        Ok(resolved)
    } else {
        Err(AppError::new(
            ErrorCode::NotFound,
            format!(
                "fixture '{}' not found under {}",
                rel.as_ref().display(),
                root().display()
            ),
        ))
    }
}

/// Read a UTF-8 config fixture under `fixtures/config/<rel>`.
///
/// `rel` is relative to the `config/` subtree, e.g. `"valid/single-rust.toml"`.
pub fn document_string(rel: impl AsRef<Path>) -> AppResult<String> {
    let resolved = existing(Path::new("config").join(rel.as_ref()))?;
    file::read_string(&resolved)
}

/// Read and parse a config fixture into a [`toml::Value`].
///
/// `rel` is relative to the `config/` subtree, e.g. `"valid/single-rust.toml"`.
pub fn document(rel: impl AsRef<Path>) -> AppResult<toml::Value> {
    let raw = document_string(rel.as_ref())?;
    toml::from_str(&raw)
        .map_err(|error| AppError::invalid_input("fixture_document", error.to_string()))
}

/// Read a UTF-8 ecosystem-specific fixture under `fixtures/ecosystems/<id>/<rel>`.
///
/// Ecosystem fixtures are isolated per id: adding a new ecosystem never edits
/// another ecosystem's files.
pub fn ecosystem_string(id: &str, rel: impl AsRef<Path>) -> AppResult<String> {
    let resolved = existing(Path::new("ecosystems").join(id).join(rel.as_ref()))?;
    file::read_string(&resolved)
}

/// Resolve an ecosystem-specific fixture path under
/// `fixtures/ecosystems/<id>/<rel>` (e.g. a sample workspace directory).
pub fn ecosystem(id: &str, rel: impl AsRef<Path>) -> AppResult<PathBuf> {
    existing(Path::new("ecosystems").join(id).join(rel.as_ref()))
}

/// Resolve the path to a sample repo tree under `fixtures/repos/<name>`.
///
/// The returned directory is the source the [`SampleRepo`](crate::repo::SampleRepo)
/// builder copies into a temp workspace.
pub fn repo_path(name: &str) -> AppResult<PathBuf> {
    let resolved = existing(Path::new("repos").join(name))?;
    if dir::exists(&resolved)? {
        Ok(resolved)
    } else {
        Err(AppError::invalid_input(
            "fixture_repo",
            format!("repo fixture '{name}' is not a directory"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use rskit_errors::ErrorCode;

    use super::{document, document_string, ecosystem, path, repo_path};

    #[test]
    fn rejects_traversing_paths() {
        let error = path("../escape.toml").unwrap_err();
        assert_eq!(error.code(), ErrorCode::InvalidInput);
    }

    #[test]
    fn missing_document_is_clear_not_found() {
        let error = document_string("valid/does-not-exist.toml").unwrap_err();
        assert_eq!(error.code(), ErrorCode::NotFound);
        assert!(error.message().contains("not found"));
    }

    #[test]
    fn loads_and_parses_valid_document() {
        let value = document("valid/single-rust.toml").expect("loads fixture");
        assert!(value.get("project").is_some());
    }

    #[test]
    fn loads_ecosystem_fixture() {
        let adapter = ecosystem("rust", "adapter/cargo.toml").expect("loads ecosystem fixture");
        assert!(adapter.exists());
    }

    #[test]
    fn resolves_repo_directory() {
        let repo = repo_path("single-rust").expect("resolves repo");
        assert!(repo.join("toven.toml").exists());
    }
}
