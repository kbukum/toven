//! Cache path resolution.

use std::{ffi::OsString, path::PathBuf};

use crate::{
    cache::decision::CACHE_DIRECTORY,
    core::{AppError, AppResult, CacheLocation, Workspace},
};

/// Environment variable that overrides the configured cache root.
pub const TOVEN_CACHE_DIR_ENV: &str = "TOVEN_CACHE_DIR";

const APP_NAME: &str = "toven";

/// Resolve the task cache root for a workspace.
pub fn resolve_task_cache_root(workspace: &Workspace) -> AppResult<PathBuf> {
    resolve_task_cache_root_from_env(workspace, |key| std::env::var_os(key))
}

fn resolve_task_cache_root_from_env(
    workspace: &Workspace,
    env: impl Fn(&str) -> Option<OsString>,
) -> AppResult<PathBuf> {
    if let Some(override_root) = env(TOVEN_CACHE_DIR_ENV) {
        return env_override_cache_root(override_root);
    }
    let root = match workspace.cache.location {
        CacheLocation::User => rskit_fs::app_cache_dir(APP_NAME)?
            .join("workspaces")
            .join(workspace_cache_id(workspace)),
        CacheLocation::Workspace => workspace.root.join(".toven/cache"),
    };
    Ok(root.join(CACHE_DIRECTORY))
}

fn env_override_cache_root(value: OsString) -> AppResult<PathBuf> {
    if value.as_os_str().is_empty() {
        return Err(AppError::invalid_input(
            TOVEN_CACHE_DIR_ENV,
            "cache directory override cannot be empty",
        ));
    }
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(AppError::invalid_input(
            TOVEN_CACHE_DIR_ENV,
            "cache directory override must be an absolute path",
        ));
    }
    Ok(path.join(CACHE_DIRECTORY))
}

fn workspace_cache_id(workspace: &Workspace) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"toven-workspace-cache-v1");
    hasher.update(workspace.root.to_string_lossy().as_bytes());
    hasher.finalize().to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, ffi::OsString, path::Path};

    use crate::core::{CacheLocation, CacheSettings, Workspace};

    use super::{TOVEN_CACHE_DIR_ENV, resolve_task_cache_root_from_env};

    #[test]
    fn env_override_wins_and_keeps_format_version() {
        let workspace = workspace(CacheLocation::User);
        let mut env = BTreeMap::new();
        env.insert(TOVEN_CACHE_DIR_ENV, OsString::from("/tmp/toven-cache"));

        assert_eq!(
            resolve_task_cache_root_from_env(&workspace, |key| env.get(key).cloned()).unwrap(),
            Path::new("/tmp/toven-cache/v3")
        );
    }

    #[test]
    fn workspace_location_uses_workspace_local_cache() {
        let workspace = workspace(CacheLocation::Workspace);
        let env = BTreeMap::<&str, OsString>::new();

        assert_eq!(
            resolve_task_cache_root_from_env(&workspace, |key| env.get(key).cloned()).unwrap(),
            Path::new("/repo/.toven/cache/v3")
        );
    }

    #[test]
    fn env_override_must_be_absolute() {
        let workspace = workspace(CacheLocation::User);
        let mut env = BTreeMap::new();
        env.insert(TOVEN_CACHE_DIR_ENV, OsString::from("relative"));

        assert!(resolve_task_cache_root_from_env(&workspace, |key| env.get(key).cloned()).is_err());
    }

    fn workspace(location: CacheLocation) -> Workspace {
        Workspace {
            schema: 1,
            name: "demo".to_string(),
            root: Path::new("/repo").to_path_buf(),
            base_ref: None,
            cache: CacheSettings { location },
            profiles: Vec::new(),
            dependency_overlays: Vec::new(),
        }
    }
}
