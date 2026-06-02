//! Root-relative Git ignore checks shared by discovery and search flows.

use std::{
    fmt,
    path::{Path, PathBuf},
};

use rskit_git::{IgnoreReader, Repository};

use crate::core::{AppError, AppResult};

/// Root-relative Git ignore matcher.
pub(crate) struct GitIgnore {
    repo: rskit_git::Repo,
    root_prefix: PathBuf,
}

impl GitIgnore {
    /// Discover Git ignore rules for `root`.
    pub(crate) fn discover(root: &Path) -> AppResult<Option<Self>> {
        let repo = match rskit_git::discover(root) {
            Ok(repo) => repo,
            Err(error) if error.is_not_found() => return Ok(None),
            Err(error) => {
                return Err(AppError::invalid_input(
                    "workspace.root",
                    format!("failed to inspect git repository for '{}'", root.display()),
                )
                .with_cause(error));
            }
        };
        let root_prefix = rskit_git::repo_relative_path(repo.root(), root).map_err(|error| {
            AppError::invalid_input(
                "workspace.root",
                format!(
                    "failed to resolve '{}' relative to git root '{}'",
                    root.display(),
                    repo.root().display()
                ),
            )
            .with_cause(error)
        })?;
        Ok(Some(Self { repo, root_prefix }))
    }

    /// Reports whether `root_relative_path` is ignored by Git.
    pub(crate) fn is_ignored(&self, root_relative_path: &Path) -> AppResult<bool> {
        let root_relative_path = normalize_git_path(root_relative_path);
        let repo_relative_path = rskit_git::join_repo_path(
            &self.root_prefix,
            Path::new(&root_relative_path),
        )
        .map_err(|error| {
            AppError::invalid_input(
                "path",
                format!("failed to resolve path '{root_relative_path}' relative to git root"),
            )
            .with_cause(error)
        })?;
        let repo_relative_path = normalize_git_path(&repo_relative_path);
        self.repo.is_ignored(&repo_relative_path).map_err(|error| {
            AppError::invalid_input(
                "path",
                format!("failed to inspect git ignore status for '{repo_relative_path}'"),
            )
            .with_cause(error)
        })
    }
}

fn normalize_git_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

impl fmt::Debug for GitIgnore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GitIgnore")
            .field("root_prefix", &self.root_prefix)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::normalize_git_path;

    #[test]
    fn renders_git_paths_with_forward_slashes() {
        assert_eq!(
            normalize_git_path(Path::new("tools\\target\\Cargo.toml")),
            "tools/target/Cargo.toml"
        );
    }
}
