//! The on-disk content cache backend shared by PLAN and APPLY.
//!
//! [`FsContentCache`] is a content-addressed *presence* cache: a unit's content
//! key (a BLAKE3 hex digest produced during PLAN) maps to a small marker file,
//! and a record's existence means "this content was already built". It stores
//! no build outputs — the key folds dependency source hashes, so a HIT is safe
//! to skip — which keeps the backend a thin, auditable layer over `rskit-fs`
//! atomic writes. Because a HIT restores nothing, the marker also holds a
//! fingerprint of the unit's declared outputs, and a record whose outputs are
//! now missing or rebuilt is not a HIT.
//!
//! One type implements both halves of the cache seam: the read-only
//! [`CacheStore`] queried by the pure synchronous planner and the write-only
//! [`CacheWriter`] driven by APPLY after a unit succeeds. Both are synchronous,
//! matching the ports, so no async runtime is bridged into PLAN.

use std::path::{Path, PathBuf};

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::sync_io::file;
use rskit_util::hash::hash_hex;
use toven_model::{AbsPath, RepoPath};
use toven_ports::{CacheStore, CacheWriter};

use crate::source::outputs_fingerprint;

/// Temp-file prefix used for the marker's atomic write.
const MARKER_TEMP_PREFIX: &str = "toven-cache";

/// Upper bound on a marker file read, guarding the key-collision check against
/// a corrupt or hostile oversized file. Markers hold a key and an outputs
/// fingerprint, so this is ample.
const MAX_MARKER_BYTES: u64 = 64 * 1024;

/// A filesystem-backed, content-addressed presence cache.
///
/// Records live under `root` in a two-character shard directory derived from
/// the key's own digest, keeping any single directory small. Declared outputs
/// resolve under `project_root`.
#[derive(Debug, Clone)]
pub struct FsContentCache {
    root: PathBuf,
    project_root: PathBuf,
}

impl FsContentCache {
    /// Create a cache rooted at `root` for the project at `project_root`. The
    /// directory is created lazily on the first [`record`](CacheWriter::record).
    #[must_use]
    pub fn new(root: impl Into<PathBuf>, project_root: &AbsPath) -> Self {
        Self {
            root: root.into(),
            project_root: project_root.as_path().to_path_buf(),
        }
    }

    /// The sharded marker path for `key`.
    ///
    /// The key is digested (not used verbatim) so an arbitrary key string can
    /// never escape `root` via path separators or `..` segments.
    fn marker_path(&self, key: &str) -> PathBuf {
        let digest = hash_hex(key.as_bytes());
        self.root.join(&digest[..2]).join(digest)
    }
}

impl CacheStore for FsContentCache {
    fn contains(&self, key: &str, outputs: &[RepoPath]) -> AppResult<bool> {
        let path = self.marker_path(key);
        if !file::exists(&path)? {
            return Ok(false);
        }
        // The marker records the key it was written for; a mismatch means two distinct
        // keys digested to the same shard path (astronomically unlikely with BLAKE3)
        // and must fail loudly rather than alias a HIT.
        let stored = file::read_string_bounded(&path, MAX_MARKER_BYTES)?;
        let (stored_key, recorded) = stored.split_once('\n').unwrap_or((stored.as_str(), ""));
        if stored_key != key {
            return Err(marker_collision_error(&path));
        }
        let current = outputs_fingerprint(&self.project_root, outputs)?;
        Ok(current.as_deref() == Some(recorded))
    }
}

impl CacheWriter for FsContentCache {
    fn record(&self, key: &str, outputs: &[RepoPath]) -> AppResult<()> {
        // A run that left a declared output missing is not reusable.
        let Some(fingerprint) = outputs_fingerprint(&self.project_root, outputs)? else {
            return Ok(());
        };
        let path = self.marker_path(key);
        file::create_parent_dir(&path)?;
        file::write_atomic(
            &path,
            format!("{key}\n{fingerprint}").as_bytes(),
            MARKER_TEMP_PREFIX,
        )
    }
}

fn marker_collision_error(path: &Path) -> AppError {
    AppError::new(
        ErrorCode::Conflict,
        format!("cache marker key collision for '{}'", path.display()),
    )
}

#[cfg(test)]
mod tests {
    use rskit_fs::TempDir;
    use rskit_fs::sync_io::file;
    use toven_model::{AbsPath, RepoPath};
    use toven_ports::{CacheStore, CacheWriter};

    use super::{FsContentCache, MAX_MARKER_BYTES};

    struct Fixture {
        _cache_dir: TempDir,
        project: TempDir,
        cache: FsContentCache,
    }

    fn fixture() -> Fixture {
        let cache_dir = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        let root = AbsPath::new(project.path()).unwrap();
        let cache = FsContentCache::new(cache_dir.path(), &root);
        Fixture {
            _cache_dir: cache_dir,
            project,
            cache,
        }
    }

    fn dist() -> Vec<RepoPath> {
        vec![RepoPath::new("pkg/dist").unwrap()]
    }

    fn write_output(fixture: &Fixture, name: &str, body: &str) {
        let path = fixture.project.path().join("pkg/dist").join(name);
        file::create_parent_dir(&path).unwrap();
        file::write(&path, body).unwrap();
    }

    #[test]
    fn miss_then_hit_after_record() {
        let fixture = fixture();
        let cache = &fixture.cache;

        assert!(!cache.contains("key-a", &[]).unwrap());
        cache.record("key-a", &[]).unwrap();
        assert!(cache.contains("key-a", &[]).unwrap());
        // A distinct key is independent.
        assert!(!cache.contains("key-b", &[]).unwrap());
    }

    #[test]
    fn record_is_idempotent() {
        let fixture = fixture();
        let cache = &fixture.cache;

        cache.record("key", &[]).unwrap();
        cache.record("key", &[]).unwrap();
        assert!(cache.contains("key", &[]).unwrap());
    }

    #[test]
    fn hit_requires_declared_outputs_to_be_present_and_unchanged() {
        let fixture = fixture();
        write_output(&fixture, "index.js", "one");
        fixture.cache.record("key", &dist()).unwrap();
        assert!(fixture.cache.contains("key", &dist()).unwrap());

        // Rebuilt output (different size) no longer matches the record.
        write_output(&fixture, "index.js", "rebuilt");
        assert!(!fixture.cache.contains("key", &dist()).unwrap());

        // Deleted output is a miss.
        fixture.cache.record("key", &dist()).unwrap();
        file::remove(&fixture.project.path().join("pkg/dist/index.js")).unwrap();
        std::fs::remove_dir(fixture.project.path().join("pkg/dist")).unwrap();
        assert!(!fixture.cache.contains("key", &dist()).unwrap());
    }

    #[test]
    fn run_with_missing_outputs_is_not_recorded() {
        let fixture = fixture();
        fixture.cache.record("key", &dist()).unwrap();
        assert!(!file::exists(&fixture.cache.marker_path("key")).unwrap());
    }

    #[test]
    fn marker_with_mismatched_key_is_a_conflict() {
        let fixture = fixture();
        let cache = &fixture.cache;
        let path = cache.marker_path("key");
        file::create_parent_dir(&path).unwrap();
        file::write_atomic(&path, b"some-other-key\n", "toven-cache-test").unwrap();

        let error = cache
            .contains("key", &[])
            .expect_err("a key mismatch at the shard path must fail loudly");
        assert_eq!(error.code(), rskit_errors::ErrorCode::Conflict);
    }

    #[test]
    fn oversized_marker_file_is_rejected() {
        let fixture = fixture();
        let cache = &fixture.cache;
        let path = cache.marker_path("key");
        file::create_parent_dir(&path).unwrap();
        let oversized = vec![b'x'; usize::try_from(MAX_MARKER_BYTES).unwrap() + 1];
        file::write_atomic(&path, oversized, "toven-cache-test").unwrap();

        assert!(cache.contains("key", &[]).is_err());
    }
}
