//! The content-digest port: per-module and per-file content identities folded
//! into the cache key.
//!
//! Hashing is a filesystem side effect, so it is an injected port: the planner
//! stays pure and tests substitute a deterministic in-memory digest. The
//! production [`FsSourceDigest`] walks the project tree with bounded reads.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rskit_errors::{AppError, AppResult};
use rskit_fs::{safe_join, sync_io::file};
use toven_model::{AbsPath, Module};

/// A stable content identity for module sources and shared-input files.
///
/// Both methods return an opaque, stable hex string that changes iff the hashed
/// content changes. A missing path hashes to a stable empty identity rather than
/// erroring, so an absent optional shared input does not abort PLAN.
pub trait SourceDigest {
    /// Content identity of a module's source tree (`module.root` subtree).
    fn module(&self, module: &Module) -> AppResult<String>;

    /// Content identity of one workspace-relative shared-input file.
    fn path(&self, repo_relative: &Path) -> AppResult<String>;
}

/// Per-file read cap (16 MiB): large enough for any source/lock file, bounded so
/// a pathological file cannot exhaust memory.
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// Cap on the number of files hashed under a single module root, bounding the
/// walk against a runaway tree.
const MAX_FILES: usize = 100_000;

/// The production [`SourceDigest`]: blake3 over the on-disk project tree.
///
/// Rooted at the absolute project root; module roots and shared-input paths are
/// joined under it with traversal-safe [`safe_join`].
#[derive(Debug, Clone)]
pub struct FsSourceDigest {
    project_root: PathBuf,
}

impl FsSourceDigest {
    /// Construct a digest rooted at `project_root`.
    #[must_use]
    pub fn new(project_root: &AbsPath) -> Self {
        Self {
            project_root: project_root.as_path().to_path_buf(),
        }
    }

    fn hash_file_into(absolute: &Path, hasher: &mut blake3::Hasher) -> AppResult<()> {
        let bytes = file::read_bounded(absolute, MAX_FILE_BYTES)?;
        hasher.update(&bytes);
        Ok(())
    }
}

impl SourceDigest for FsSourceDigest {
    fn module(&self, module: &Module) -> AppResult<String> {
        let root = safe_join(&self.project_root, module.root.as_path())
            .map_err(|error| AppError::invalid_input("module.root", error.to_string()))?;
        if !root.is_dir() {
            return Ok(empty_digest());
        }

        // Collect files relative to the module root, sorted for a stable identity
        // independent of directory-iteration order.
        let mut files = BTreeMap::new();
        collect_files(&root, &root, &mut files)?;

        let mut hasher = blake3::Hasher::new();
        for (relative, absolute) in &files {
            hasher.update(relative.to_string_lossy().as_bytes());
            hasher.update(b"\0");
            Self::hash_file_into(absolute, &mut hasher)?;
            hasher.update(b"\0");
        }
        Ok(hasher.finalize().to_hex().to_string())
    }

    fn path(&self, repo_relative: &Path) -> AppResult<String> {
        let absolute = safe_join(&self.project_root, repo_relative)
            .map_err(|error| AppError::invalid_input("shared_inputs", error.to_string()))?;
        if !file::exists(&absolute)? {
            return Ok(empty_digest());
        }
        let mut hasher = blake3::Hasher::new();
        Self::hash_file_into(&absolute, &mut hasher)?;
        Ok(hasher.finalize().to_hex().to_string())
    }
}

/// The stable identity of absent content.
fn empty_digest() -> String {
    blake3::Hasher::new().finalize().to_hex().to_string()
}

/// Recursively collect files under `dir`, keyed by their path relative to `base`.
fn collect_files(base: &Path, dir: &Path, files: &mut BTreeMap<PathBuf, PathBuf>) -> AppResult<()> {
    let entries = std::fs::read_dir(dir)
        .map_err(|error| AppError::new(rskit_errors::ErrorCode::Internal, error.to_string()))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| AppError::new(rskit_errors::ErrorCode::Internal, error.to_string()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| AppError::new(rskit_errors::ErrorCode::Internal, error.to_string()))?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_files(base, &path, files)?;
        } else if file_type.is_file() {
            if files.len() >= MAX_FILES {
                return Err(AppError::new(
                    rskit_errors::ErrorCode::Internal,
                    format!(
                        "source tree under '{}' exceeds {MAX_FILES} files",
                        base.display()
                    ),
                ));
            }
            let relative = path.strip_prefix(base).unwrap_or(&path).to_path_buf();
            files.insert(relative, path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use toven_model::{AbsPath, EcosystemId, Module, ModuleRef, RepoPath};
    use toven_testkit::TestWorkspace;

    use super::{FsSourceDigest, SourceDigest};

    fn module(root: &str) -> Module {
        Module::new(
            ModuleRef::new(EcosystemId::new("rust").unwrap(), "errors").unwrap(),
            RepoPath::new(root).unwrap(),
        )
    }

    #[test]
    fn module_digest_changes_with_content() {
        let workspace = TestWorkspace::new("source-digest");
        workspace.write_file("core/lib.rs", b"fn a() {}").unwrap();
        let root = AbsPath::new(workspace.path()).unwrap();

        let digest = FsSourceDigest::new(&root);
        let before = digest.module(&module("core")).unwrap();

        workspace.write_file("core/lib.rs", b"fn b() {}").unwrap();
        let after = digest.module(&module("core")).unwrap();
        assert_ne!(before, after);
    }

    #[test]
    fn missing_module_root_is_stable_empty() {
        let workspace = TestWorkspace::new("source-digest-empty");
        let root = AbsPath::new(workspace.path()).unwrap();
        let digest = FsSourceDigest::new(&root);
        let a = digest.module(&module("absent")).unwrap();
        let b = digest.module(&module("also-absent")).unwrap();
        assert_eq!(a, b);
    }
}
