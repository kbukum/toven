//! Path newtypes that encode the repo-relative vs. absolute trust boundary.

use std::{
    fmt,
    path::{Component, Path, PathBuf},
};

use rskit_errors::{AppError, AppResult};
use serde::{Deserialize, Serialize};

/// A repository-relative path with no traversal (`..`) or root components.
///
/// Used for every path that crosses the discovery boundary (module roots,
/// manifests, workspace roots). Rejecting absolute paths and `..` at
/// construction prevents path-escape/traversal before the value is ever joined
/// against the project root.
#[derive(Debug, Clone, Eq, Ord, PartialEq, PartialOrd, Hash, Deserialize, Serialize)]
#[serde(try_from = "PathBuf", into = "PathBuf")]
pub struct RepoPath(PathBuf);

impl RepoPath {
    /// Validate and construct a repo-relative path.
    ///
    /// `.` (`CurDir`) components are normalized away so semantically equal paths
    /// (`core/errors` and `core/./errors`) share one canonical identity; a path
    /// consisting only of `.` canonicalizes to the repo root (`.`).
    ///
    /// Errors if the path is absolute, empty, or contains a `..` / root / prefix
    /// component.
    pub fn new(path: impl Into<PathBuf>) -> AppResult<Self> {
        let path = path.into();
        if path.as_os_str().is_empty() {
            return Err(AppError::invalid_input("path", "repo path cannot be empty"));
        }
        let mut normalized = PathBuf::new();
        for component in path.components() {
            match component {
                Component::Normal(part) => normalized.push(part),
                Component::CurDir => {}
                Component::ParentDir => {
                    return Err(AppError::invalid_input(
                        "path",
                        format!("repo path '{}' must not contain '..'", path.display()),
                    ));
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(AppError::invalid_input(
                        "path",
                        format!("repo path '{}' must be relative", path.display()),
                    ));
                }
            }
        }
        // A path made up solely of `.` components denotes the repo root.
        if normalized.as_os_str().is_empty() {
            normalized.push(".");
        }
        Ok(Self(normalized))
    }

    /// Borrow the underlying path.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

impl fmt::Display for RepoPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0.display())
    }
}

impl TryFrom<PathBuf> for RepoPath {
    type Error = AppError;

    fn try_from(value: PathBuf) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<RepoPath> for PathBuf {
    fn from(value: RepoPath) -> Self {
        value.0
    }
}

/// An absolute filesystem path (e.g. the resolved project root).
#[derive(Debug, Clone, Eq, Ord, PartialEq, PartialOrd, Hash, Deserialize, Serialize)]
#[serde(try_from = "PathBuf", into = "PathBuf")]
pub struct AbsPath(PathBuf);

impl AbsPath {
    /// Validate and construct an absolute path.
    pub fn new(path: impl Into<PathBuf>) -> AppResult<Self> {
        let path = path.into();
        if !path.is_absolute() {
            return Err(AppError::invalid_input(
                "path",
                format!("path '{}' must be absolute", path.display()),
            ));
        }
        Ok(Self(path))
    }

    /// Borrow the underlying path.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

impl fmt::Display for AbsPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0.display())
    }
}

impl TryFrom<PathBuf> for AbsPath {
    type Error = AppError;

    fn try_from(value: PathBuf) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<AbsPath> for PathBuf {
    fn from(value: AbsPath) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::{AbsPath, RepoPath};

    #[test]
    fn repo_path_rejects_absolute_and_traversal() {
        assert!(RepoPath::new("core/errors").is_ok());
        assert!(RepoPath::new("../escape").is_err());
        assert!(RepoPath::new("core/../etc").is_err());
        assert!(RepoPath::new("").is_err());
        #[cfg(unix)]
        assert!(RepoPath::new("/abs").is_err());
    }

    #[test]
    fn repo_path_normalizes_curdir() {
        assert_eq!(
            RepoPath::new("core/./errors").unwrap(),
            RepoPath::new("core/errors").unwrap(),
        );
        let root = RepoPath::new(".").unwrap();
        assert_eq!(root.as_path(), std::path::Path::new("."));
    }

    #[test]
    fn abs_path_requires_absolute() {
        #[cfg(unix)]
        {
            assert!(AbsPath::new("/repo").is_ok());
            assert!(AbsPath::new("repo").is_err());
        }
    }
}
