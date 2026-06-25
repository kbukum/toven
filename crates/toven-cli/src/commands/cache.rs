//! The `cache stats|clean|path` maintenance verbs (cli-taxonomy namespaced
//! surface).
//!
//! All three operate on the resolved on-disk cache root for the project
//! ([`Project::cache_root`](crate::host::Project::cache_root)): `path` prints it,
//! `stats` summarizes the entries under it, and `clean` removes it. They touch
//! the filesystem directly (via rskit-fs) and never go through the PLAN spine.

use rskit_cli::{ExitCode, OutputKV};
use rskit_errors::AppResult;
use rskit_fs::sync_io::file::metadata;
use rskit_fs::sync_io::tree::{list_tree, remove_tree_if_exists};

use crate::flags::CacheAction;
use crate::host::Project;

/// Dispatch a `toven cache <action>`.
///
/// # Errors
/// Propagates cache-root resolution and filesystem failures.
pub(crate) fn execute(project: &Project, action: &CacheAction) -> AppResult<ExitCode> {
    match action {
        CacheAction::Path => path(project),
        CacheAction::Stats => stats(project),
        CacheAction::Clean => clean(project),
    }
}

/// Print the resolved cache directory.
fn path(project: &Project) -> AppResult<ExitCode> {
    println!("{}", project.cache_root()?.display());
    Ok(ExitCode::Success)
}

/// Summarize the entry and byte counts under the cache directory.
fn stats(project: &Project) -> AppResult<ExitCode> {
    let root = project.cache_root()?;
    let (files, bytes) = if root.is_dir() {
        let entries = list_tree(&root, false)?;
        let mut files = 0_usize;
        let mut bytes = 0_u64;
        for entry in entries.iter().filter(|entry| entry.is_file) {
            files += 1;
            bytes += metadata(&entry.path)?.len;
        }
        (files, bytes)
    } else {
        (0, 0)
    };

    let mut summary = OutputKV::new();
    summary
        .add("path", root.display().to_string())
        .add("entries", files.to_string())
        .add("bytes", bytes.to_string());
    println!("{summary}");
    Ok(ExitCode::Success)
}

/// Remove the cache directory, reporting whether anything was deleted.
fn clean(project: &Project) -> AppResult<ExitCode> {
    let root = project.cache_root()?;
    let removed = remove_tree_if_exists(&root)?;
    if removed {
        println!("removed cache directory: {}", root.display());
    } else {
        println!("cache directory already absent: {}", root.display());
    }
    Ok(ExitCode::Success)
}
