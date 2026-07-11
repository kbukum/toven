//! The content-digest port: per-module and per-file content identities folded
//! into the cache key.
//!
//! Hashing is a filesystem side effect, so it is an injected port: the planner
//! stays pure and tests substitute a deterministic in-memory digest. The
//! production [`FsSourceDigest`] walks the project tree with bounded reads.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::sync_io::tree::{IgnoreWalkOptions, WalkControl, walk_tree_ignoring};
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

/// Content-hashing walk policy: honour `.gitignore`/`.ignore` rules so build
/// output (`target/`, generated dirs) never enters the digest, but keep hidden
/// configuration (`.cargo/config.toml`, `.gitignore`) because it is source that
/// affects builds. `.git` and gitignored artifacts are always excluded.
const HASH_WALK: IgnoreWalkOptions = IgnoreWalkOptions {
    respect_gitignore: true,
    skip_hidden: false,
    follow_symlinks: false,
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
        let bytes = file::read_bounded(absolute, MAX_FILE_BYTES).map_err(|error| {
            // Only the size-cap breach gets the "likely a build artifact" hint;
            // any other IO failure (missing file, permission denied, …) keeps its
            // own accurate message and cause.
            if file::is_file_too_large_error(&error) {
                AppError::invalid_input(
                    "source_file",
                    format!(
                        "file '{}' exceeds the {MAX_FILE_BYTES}-byte source-digest cap; \
                         this is usually a build artifact that should be git-ignored \
                         (e.g. under 'target/') rather than tracked source",
                        absolute.display()
                    ),
                )
                .with_cause(error)
            } else {
                error
            }
        })?;
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
            return Err(AppError::new(
                rskit_errors::ErrorCode::NotFound,
                format!(
                    "module '{}' root '{}' is not a directory; a vanished or invalid module tree cannot be cached",
                    module.id,
                    module.root.as_path().display()
                ),
            ));
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

/// Collect every source file under `root` (ignoring `.git` and gitignored build
/// output), keyed by its path relative to `root`.
///
/// Symlinks are skipped and the walk is bounded at [`MAX_FILES`]. The returned
/// [`BTreeMap`] is ordered by relative path, giving a stable identity that is
/// independent of directory-iteration order.
fn collect_files(root: &Path) -> AppResult<BTreeMap<PathBuf, PathBuf>> {
    let mut files = BTreeMap::new();
    walk_tree_ignoring(root, HASH_WALK, |entry| {
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
    fn missing_module_root_is_not_found() {
        let workspace = TestWorkspace::new("source-digest-empty");
        let root = AbsPath::new(workspace.path()).unwrap();
        let digest = FsSourceDigest::new(&root);
        let error = digest.module(&module("absent")).unwrap_err();
        assert_eq!(error.code(), rskit_errors::ErrorCode::NotFound);
    }

    #[test]
    fn ignored_build_output_is_excluded_from_digest() {
        // A large, git-ignored build artifact under `target/` must neither abort
        // planning (16 MiB cap) nor perturb the cache key when it changes.
        let workspace = TestWorkspace::new("source-digest-ignored");
        workspace
            .write_file("core/.gitignore", b"target/\n")
            .unwrap();
        workspace.write_file("core/lib.rs", b"fn a() {}").unwrap();
        let big = vec![b'x'; 18 * 1024 * 1024];
        workspace
            .write_file("core/target/release/deps/lib.rlib", &big)
            .unwrap();
        let root = AbsPath::new(workspace.path()).unwrap();

        let digest = FsSourceDigest::new(&root);
        let before = digest.module(&module("core")).unwrap();

        // Mutating the ignored artifact must not move the digest.
        let mut bigger = big;
        bigger.extend_from_slice(b"more");
        workspace
            .write_file("core/target/release/deps/lib.rlib", &bigger)
            .unwrap();
        let after = digest.module(&module("core")).unwrap();
        assert_eq!(before, after, "ignored artifact churn must not move digest");

        // But a real source change still moves it.
        workspace.write_file("core/lib.rs", b"fn b() {}").unwrap();
        let changed = digest.module(&module("core")).unwrap();
        assert_ne!(before, changed);
    }

    #[test]
    fn oversize_tracked_file_reports_the_source_digest_cap() {
        // A large *tracked* file (not git-ignored) still trips the cap, and the
        // error must carry the build-artifact hint so the fix is actionable.
        let workspace = TestWorkspace::new("source-digest-oversize");
        workspace.write_file("core/lib.rs", b"fn a() {}").unwrap();
        workspace
            .write_file("core/huge.bin", &vec![b'x'; 18 * 1024 * 1024])
            .unwrap();
        let root = AbsPath::new(workspace.path()).unwrap();

        let error = FsSourceDigest::new(&root)
            .module(&module("core"))
            .unwrap_err();
        assert_eq!(error.code(), rskit_errors::ErrorCode::InvalidInput);
        assert!(
            error.to_string().contains("source-digest cap"),
            "oversize tracked file keeps the cap hint, got: {error}"
        );
    }

    #[test]
    fn non_oversize_read_failure_is_not_mislabeled_as_oversize() {
        // A directory is not a regular file, so the bounded read fails with a
        // distinct error that must surface as-is, never disguised as a size-cap
        // breach.
        use rskit_util::hash::ContentHasher;

        let workspace = TestWorkspace::new("source-digest-nonregular");
        workspace.write_file("pkg/keep.rs", b"fn a() {}").unwrap();
        let dir = workspace.path().join("pkg");

        let mut hasher = ContentHasher::new();
        let error = FsSourceDigest::hash_file_into(&dir, &mut hasher).unwrap_err();
        assert!(
            !error.to_string().contains("source-digest cap"),
            "a non-oversize failure must not claim the size cap, got: {error}"
        );
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
