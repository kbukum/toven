//! The content-digest port: per-module and per-file content identities folded
//! into the cache key.
//!
//! Hashing is a filesystem side effect, so it is an injected port: the planner
//! stays pure and tests substitute a deterministic in-memory digest. The
//! production [`FsSourceDigest`] walks the project tree with bounded reads.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::sync_io::tree::{WalkControl, WalkEntryFilter, WalkOptions, walk_tree};
use rskit_fs::{safe_join, sync_io::file};
use rskit_util::hash::ContentHasher;
use toven_model::{AbsPath, Module};
use toven_ports::SourceDigest;

/// Per-file read cap (16 MiB): large enough for any source/lock file, bounded so
/// a pathological file cannot exhaust memory.
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// Cap on the number of files hashed under a single module root, bounding the
/// walk against a runaway tree.
const MAX_FILES: usize = 100_000;

/// Content-hashing walk policy: visit regular files only and never follow
/// symlinks, so a symlinked file or directory cannot leak content from outside
/// the tree or cause an unbounded traversal.
const HASH_WALK: WalkOptions = WalkOptions {
    follow_symlinks: false,
    entry_filter: WalkEntryFilter::FILES,
};

/// The production [`SourceDigest`]: a BLAKE3 content hash (via
/// `rskit_util::hash`) over the on-disk project tree.
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

    /// Read one file's bounded contents and fold it into `hasher` as a framed
    /// `content` field.
    fn hash_file_into(absolute: &Path, hasher: &mut ContentHasher) -> AppResult<()> {
        let bytes = file::read_bounded(absolute, MAX_FILE_BYTES)?;
        hasher.update_framed(b"content", &bytes);
        Ok(())
    }

    /// Hash a directory subtree as a stable, order-independent identity.
    fn hash_tree(root: &Path) -> AppResult<String> {
        // Collect files relative to the root, sorted for a stable identity
        // independent of directory-iteration order.
        let files = collect_files(root)?;

        let mut hasher = ContentHasher::new();
        for (relative, absolute) in &files {
            hasher.update_framed(b"path", relative.to_string_lossy().as_bytes());
            Self::hash_file_into(absolute, &mut hasher)?;
        }
        Ok(hasher.finalize_hex())
    }
}

impl SourceDigest for FsSourceDigest {
    fn module(&self, module: &Module) -> AppResult<String> {
        let root = safe_join(&self.project_root, module.root.as_path()).map_err(|error| {
            AppError::invalid_input("module.root", error.to_string()).with_cause(error)
        })?;
        if !root.is_dir() {
            return Ok(empty_digest());
        }
        Self::hash_tree(&root)
    }

    fn path(&self, repo_relative: &Path) -> AppResult<String> {
        let absolute = safe_join(&self.project_root, repo_relative).map_err(|error| {
            AppError::invalid_input("shared_inputs", error.to_string()).with_cause(error)
        })?;
        // A shared input may be a file or a directory; `file::exists` is true
        // only for regular files, so directories are detected separately and
        // hashed as a subtree, matching the documented file-or-directory shape.
        if absolute.is_dir() {
            return Self::hash_tree(&absolute);
        }
        if !file::exists(&absolute)? {
            return Ok(empty_digest());
        }
        let mut hasher = ContentHasher::new();
        Self::hash_file_into(&absolute, &mut hasher)?;
        Ok(hasher.finalize_hex())
    }
}

/// The stable identity of absent content.
fn empty_digest() -> String {
    ContentHasher::new().finalize_hex()
}

/// Collect every regular file under `root`, keyed by its path relative to
/// `root`, by walking the tree with rskit-fs.
///
/// Symlinks are skipped and the walk is bounded at [`MAX_FILES`]. The returned
/// [`BTreeMap`] is ordered by relative path, giving a stable identity that is
/// independent of directory-iteration order.
fn collect_files(root: &Path) -> AppResult<BTreeMap<PathBuf, PathBuf>> {
    let mut files = BTreeMap::new();
    walk_tree(root, HASH_WALK, |entry| {
        if files.len() >= MAX_FILES {
            return Err(AppError::new(
                ErrorCode::Internal,
                format!(
                    "source tree under '{}' exceeds {MAX_FILES} files",
                    root.display()
                ),
            ));
        }
        files.insert(entry.relative_path.clone(), entry.path.clone());
        Ok(WalkControl::Continue)
    })?;
    Ok(files)
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

    #[test]
    fn directory_shared_input_hashes_its_subtree() {
        let workspace = TestWorkspace::new("source-digest-dir");
        workspace
            .write_file("shared/schema.sql", b"create table a();")
            .unwrap();
        let root = AbsPath::new(workspace.path()).unwrap();
        let digest = FsSourceDigest::new(&root);

        // A directory shared input is hashed as a subtree, so a change under it
        // moves the digest (a regression for `file::exists` rejecting dirs).
        let before = digest.path(std::path::Path::new("shared")).unwrap();
        let empty = digest.path(std::path::Path::new("absent")).unwrap();
        assert_ne!(before, empty, "a populated dir must not hash as empty");

        workspace
            .write_file("shared/schema.sql", b"create table b();")
            .unwrap();
        let after = digest.path(std::path::Path::new("shared")).unwrap();
        assert_ne!(before, after);
    }
}
