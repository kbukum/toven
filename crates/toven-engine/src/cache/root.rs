//! Cache-root resolution shared by the run wiring and the `cache` CLI verbs.
//!
//! The on-disk [`FsContentCache`](super::FsContentCache) is rooted at a directory
//! resolved by a fixed precedence so a run and `toven cache path` always agree on
//! where records live. Resolution is pure orchestration (env + config + the
//! platform user-cache directory), so it lives in the engine — the CLI only
//! renders the result, it never re-derives the path.

use std::path::PathBuf;

use rskit_errors::{AppError, AppResult};
use rskit_fs::{app_cache_dir, safe_join};
use rskit_util::hash::hash_hex;
use toven_model::AbsPath;

/// Environment override for the cache base directory (absolute path).
pub const CACHE_DIR_ENV: &str = "TOVEN_CACHE_DIR";

/// Task-cache record/key format version, isolated in its own directory segment so
/// an incompatible future format starts a fresh tree instead of misreading old
/// records.
pub const CACHE_FORMAT_VERSION: &str = "v3";

/// Application name used to derive the platform user-cache directory.
const APP_NAME: &str = "toven";

/// Resolve the cache root for a workspace rooted at `project_root`.
///
/// Precedence (highest first):
/// 1. the [`CACHE_DIR_ENV`] environment override (absolute base), then the
///    format-version segment;
/// 2. a workspace-relative `[toven.cache].dir` (`configured_dir`), confined under
///    `project_root`, then the format-version segment;
/// 3. the platform user-cache directory for `toven`, namespaced by a stable hash
///    of the workspace root so distinct workspaces never share records, then the
///    format-version segment.
///
/// # Errors
/// Propagates a relative [`CACHE_DIR_ENV`] override, malformed workspace-relative
/// `configured_dir` (path traversal), or a platform user-cache-directory
/// resolution failure.
pub fn resolve_root(project_root: &AbsPath, configured_dir: Option<&str>) -> AppResult<PathBuf> {
    resolve_root_with(project_root, configured_dir, || {
        rskit_util::env::get_non_empty(CACHE_DIR_ENV)
    })
}

/// Resolve the cache root with an injected environment override accessor.
///
/// Keeps the precedence logic deterministically testable without mutating process
/// environment state.
fn resolve_root_with(
    project_root: &AbsPath,
    configured_dir: Option<&str>,
    env_override: impl Fn() -> Option<String>,
) -> AppResult<PathBuf> {
    if let Some(base) = env_override() {
        let base = PathBuf::from(&base);
        if !base.is_absolute() {
            return Err(AppError::invalid_input(
                CACHE_DIR_ENV,
                format!("{CACHE_DIR_ENV} must be an absolute path"),
            ));
        }
        return Ok(base.join(CACHE_FORMAT_VERSION));
    }
    if let Some(dir) = configured_dir.filter(|dir| !dir.is_empty()) {
        let base = safe_join(project_root.as_path(), dir).map_err(|error| {
            AppError::invalid_input(
                "toven.cache.dir",
                format!("cache dir '{dir}' escapes the project root: {error}"),
            )
        })?;
        return Ok(base.join(CACHE_FORMAT_VERSION));
    }
    let digest = hash_hex(project_root.as_path().to_string_lossy().as_bytes());
    let workspace_id = &digest[..16];
    Ok(app_cache_dir(APP_NAME)?
        .join(workspace_id)
        .join(CACHE_FORMAT_VERSION))
}

#[cfg(test)]
mod tests {
    use super::{CACHE_FORMAT_VERSION, resolve_root_with};
    use toven_model::AbsPath;

    fn root() -> AbsPath {
        AbsPath::new(if cfg!(windows) { r"C:\repo" } else { "/repo" }).expect("absolute")
    }

    fn cache_override() -> &'static str {
        if cfg!(windows) {
            r"C:\toven-cache"
        } else {
            "/tmp/toven-cache"
        }
    }

    #[test]
    fn env_override_wins_and_appends_format_version() {
        let resolved = resolve_root_with(&root(), Some("local"), || {
            Some(cache_override().to_string())
        })
        .expect("resolved");
        assert!(resolved.ends_with(CACHE_FORMAT_VERSION));
        assert!(resolved.starts_with(cache_override()));
    }

    #[test]
    fn relative_env_override_is_rejected() {
        assert!(resolve_root_with(&root(), None, || Some("relative/cache".to_string())).is_err());
    }

    #[test]
    fn configured_dir_is_confined_under_the_workspace_root() {
        let resolved = resolve_root_with(&root(), Some(".toven/cache"), || None).expect("resolved");
        assert!(resolved.starts_with(root().as_path()));
        assert!(resolved.ends_with(CACHE_FORMAT_VERSION));
    }

    #[test]
    fn traversal_in_configured_dir_is_rejected() {
        assert!(resolve_root_with(&root(), Some("../escape"), || None).is_err());
    }

    #[test]
    fn default_falls_back_to_the_platform_user_cache_directory() {
        let resolved = resolve_root_with(&root(), None, || None).expect("resolved");
        assert!(resolved.ends_with(CACHE_FORMAT_VERSION));
        assert!(!resolved.starts_with(root().as_path()));
    }
}
