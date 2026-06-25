//! The on-disk content cache backend shared by PLAN and APPLY.
//!
//! [`FsContentCache`] is a content-addressed *presence* cache: a unit's content
//! key (a BLAKE3 hex digest produced during PLAN) maps to a small marker file,
//! and a record's mere existence means "this content was already built". It
//! stores no build outputs — the key folds dependency source hashes, so a HIT
//! is safe to skip — which keeps the backend a thin, auditable layer over
//! `rskit-fs` atomic writes.
//!
//! One type implements both halves of the cache seam: the read-only
//! [`CacheStore`] queried by the pure synchronous planner and the write-only
//! [`CacheWriter`] driven by APPLY after a unit succeeds. Both are synchronous,
//! matching the ports, so no async runtime is bridged into PLAN.

use std::path::{Path, PathBuf};

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::sync_io::file;
use rskit_util::hash::hash_hex;
use toven_ports::{CacheStore, CacheWriter};

/// Temp-file prefix used for the marker's atomic write.
const MARKER_TEMP_PREFIX: &str = "toven-cache";

/// Upper bound on a marker file read, guarding the key-collision check against a
/// corrupt or hostile oversized file. Markers hold only a key, so this is ample.
const MAX_MARKER_BYTES: u64 = 64 * 1024;

/// A filesystem-backed, content-addressed presence cache.
///
/// Records live under `root` in a two-character shard directory derived from the
/// key's own digest, keeping any single directory small.
#[derive(Debug, Clone)]
pub struct FsContentCache {
    root: PathBuf,
}

impl FsContentCache {
    /// Create a cache rooted at `root`. The directory is created lazily on the
    /// first [`record`](CacheWriter::record).
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
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
    fn contains(&self, key: &str) -> AppResult<bool> {
        let path = self.marker_path(key);
        if !file::exists(&path)? {
            return Ok(false);
        }
        // The marker records the key it was written for; a mismatch means two
        // distinct keys digested to the same shard path (astronomically
        // unlikely with BLAKE3) and must fail loudly rather than alias a HIT.
        let stored = file::read_string_bounded(&path, MAX_MARKER_BYTES)?;
        if stored == key {
            Ok(true)
        } else {
            Err(marker_collision_error(&path))
        }
    }
}

impl CacheWriter for FsContentCache {
    fn record(&self, key: &str) -> AppResult<()> {
        let path = self.marker_path(key);
        file::create_parent_dir(&path)?;
        file::write_atomic(&path, key.as_bytes(), MARKER_TEMP_PREFIX)
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
    use toven_ports::{CacheStore, CacheWriter};

    use super::{FsContentCache, MAX_MARKER_BYTES};

    #[test]
    fn miss_then_hit_after_record() {
        let root = TempDir::new().unwrap();
        let cache = FsContentCache::new(root.path());

        assert!(!cache.contains("key-a").unwrap());
        cache.record("key-a").unwrap();
        assert!(cache.contains("key-a").unwrap());
        // A distinct key is independent.
        assert!(!cache.contains("key-b").unwrap());
    }

    #[test]
    fn record_is_idempotent() {
        let root = TempDir::new().unwrap();
        let cache = FsContentCache::new(root.path());

        cache.record("key").unwrap();
        cache.record("key").unwrap();
        assert!(cache.contains("key").unwrap());
    }

    #[test]
    fn marker_with_mismatched_key_is_a_conflict() {
        let root = TempDir::new().unwrap();
        let cache = FsContentCache::new(root.path());
        let path = cache.marker_path("key");
        file::create_parent_dir(&path).unwrap();
        file::write_atomic(&path, b"some-other-key", "toven-cache-test").unwrap();

        let error = cache
            .contains("key")
            .expect_err("a key mismatch at the shard path must fail loudly");
        assert_eq!(error.code(), rskit_errors::ErrorCode::Conflict);
    }

    #[test]
    fn oversized_marker_file_is_rejected() {
        let root = TempDir::new().unwrap();
        let cache = FsContentCache::new(root.path());
        let path = cache.marker_path("key");
        file::create_parent_dir(&path).unwrap();
        let oversized = vec![b'x'; usize::try_from(MAX_MARKER_BYTES).unwrap() + 1];
        file::write_atomic(&path, oversized, "toven-cache-test").unwrap();

        assert!(cache.contains("key").is_err());
    }
}
