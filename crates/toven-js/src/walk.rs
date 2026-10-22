//! The bounded walk that finds every candidate `package.json` directory.
//!
//! One pass over the project tree collects the directories holding a
//! `package.json`. It never enters `node_modules`, hidden directories, the
//! configured `exclude` directories, or directories Git ignores, and it is
//! bounded in depth and entry count so a runaway tree fails fast instead of
//! stalling planning. Ignore rules are read in process through the embedded
//! [`rskit_git`] repository; no `git` subprocess runs.

use std::path::{Component, Path, PathBuf};

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::sync_io::tree::{WalkControl, WalkOptions, walk_tree};
use rskit_git::{IgnoreReader, Repo, Repository};

use crate::manifest::PACKAGE_JSON;

/// The install directory the walk never enters.
const NODE_MODULES: &str = "node_modules";

/// Deepest directory the walk enters, relative to the project root.
const MAX_DEPTH: usize = 16;

/// Cap on entries visited, bounding the walk against a runaway tree.
const MAX_ENTRIES: usize = 200_000;

/// The repo-relative directories (empty for the project root) that hold a
/// `package.json`, sorted so every parent precedes its children.
///
/// # Errors
/// An `exclude` entry that escapes the project root, a filesystem or Git
/// ignore-rule failure, or a tree larger than the walk bound.
pub(crate) fn package_dirs(project_root: &Path, exclude: &[String]) -> AppResult<Vec<PathBuf>> {
    let exclude = exclude
        .iter()
        .map(|entry| normalize_exclude(entry))
        .collect::<AppResult<Vec<_>>>()?;
    let ignores = Ignores::open(project_root)?;

    let mut dirs = Vec::new();
    let mut visited = 0_usize;
    let mut failure = None;
    walk_tree(project_root, WalkOptions::default(), |entry| {
        visited += 1;
        if visited > MAX_ENTRIES {
            failure = Some(AppError::invalid_input(
                "ecosystems.js",
                format!(
                    "the JavaScript discovery walk visited more than {MAX_ENTRIES} entries; add \
                     the large directories to [ecosystems.js] exclude or list roots explicitly"
                ),
            ));
            return Ok(WalkControl::Stop);
        }
        let relative = &entry.relative_path;
        if entry.is_symlink {
            return Ok(WalkControl::SkipSubtree);
        }
        if entry.is_dir {
            if skip_dir(relative, &exclude) {
                return Ok(WalkControl::SkipSubtree);
            }
            match ignores.is_ignored(relative) {
                Ok(true) => return Ok(WalkControl::SkipSubtree),
                Ok(false) => {}
                Err(error) => {
                    failure = Some(error);
                    return Ok(WalkControl::Stop);
                }
            }
            return Ok(WalkControl::Continue);
        }
        if entry.is_file && relative.file_name() == Some(PACKAGE_JSON.as_ref()) {
            dirs.push(relative.parent().map(Path::to_path_buf).unwrap_or_default());
        }
        Ok(WalkControl::Continue)
    })?;
    if let Some(error) = failure {
        return Err(error);
    }
    dirs.sort();
    Ok(dirs)
}

/// Whether the walk must not enter the directory at `relative`.
fn skip_dir(relative: &Path, exclude: &[PathBuf]) -> bool {
    let name = relative.file_name().map(|name| name.to_string_lossy());
    name.is_some_and(|name| name == NODE_MODULES || name.starts_with('.'))
        || relative.components().count() > MAX_DEPTH
        || exclude
            .iter()
            .any(|excluded| relative.starts_with(excluded))
}

/// Normalize one `exclude` entry to a repo-relative directory.
fn normalize_exclude(entry: &str) -> AppResult<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in Path::new(entry).components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            _ => {
                return Err(AppError::invalid_input(
                    "ecosystems.js.exclude",
                    format!("exclude entry '{entry}' must be a directory inside the project"),
                ));
            }
        }
    }
    Ok(normalized)
}

/// Git ignore rules for the project, when it lives in a Git work tree.
struct Ignores {
    /// The repository and the project root's path inside it.
    repo: Option<(Repo, PathBuf)>,
}

impl Ignores {
    /// Open the work tree enclosing `project_root`; outside Git nothing is
    /// ignored.
    fn open(project_root: &Path) -> AppResult<Self> {
        let repo = match rskit_git::discover(project_root) {
            Ok(repo) => repo,
            Err(error) if error.code() == ErrorCode::NotFound => return Ok(Self { repo: None }),
            Err(error) => return Err(error),
        };
        let prefix = rskit_git::repo_relative_path(repo.root(), project_root)?;
        Ok(Self {
            repo: Some((repo, prefix)),
        })
    }

    /// Whether Git ignores the project-relative directory `relative`.
    fn is_ignored(&self, relative: &Path) -> AppResult<bool> {
        let Some((repo, prefix)) = &self.repo else {
            return Ok(false);
        };
        let path = prefix.join(relative);
        let path = path
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        repo.is_ignored(&path)
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::package_dirs;

    fn tree(files: &[&str]) -> rskit_fs::TempDir {
        let dir = rskit_fs::TempDir::new().expect("tempdir");
        for file in files {
            dir.write_file(file, b"{}").expect("write");
        }
        dir
    }

    fn found(dir: &Path, exclude: &[&str]) -> Vec<PathBuf> {
        let exclude: Vec<String> = exclude.iter().map(ToString::to_string).collect();
        package_dirs(dir, &exclude).expect("walk")
    }

    #[test]
    fn finds_package_dirs_parents_first_and_prunes_installs_and_hidden() {
        let dir = tree(&[
            "package.json",
            "packages/b/package.json",
            "packages/a/package.json",
            "packages/a/node_modules/dep/package.json",
            ".cache/x/package.json",
            "web/package.json",
        ]);
        assert_eq!(
            found(dir.path(), &[]),
            [
                PathBuf::new(),
                PathBuf::from("packages/a"),
                PathBuf::from("packages/b"),
                PathBuf::from("web"),
            ]
        );
    }

    #[test]
    fn exclude_prunes_directories_and_rejects_escapes() {
        let dir = tree(&["examples/demo/package.json", "web/package.json"]);
        assert_eq!(found(dir.path(), &["./examples/"]), [PathBuf::from("web")]);
        let error = package_dirs(dir.path(), &["../other".to_string()]).expect_err("escape");
        assert!(error.to_string().contains("inside the project"), "{error}");
    }

    #[test]
    fn git_ignored_directories_are_pruned() {
        let dir = tree(&["web/package.json", "out/gen/package.json"]);
        dir.write_file(".gitignore", b"out/\n").expect("gitignore");
        rskit_git::init(dir.path()).expect("git init");
        assert_eq!(found(dir.path(), &[]), [PathBuf::from("web")]);
    }
}
