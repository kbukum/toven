//! The `cache stats|clean|path` maintenance verbs (cli-taxonomy namespaced
//! surface).
//!
//! All three operate on the resolved on-disk cache root for the project
//! ([`Project::cache_root`](crate::host::Project::cache_root)): `path` prints
//! it, `stats` summarizes the entries under it, and `clean` removes it. They
//! touch the filesystem directly (via rskit-fs) and never go through the PLAN
//! spine.

use rskit_cli::{ExitCode, OutputKV};
use rskit_errors::AppResult;
use rskit_fs::sync_io::file::metadata;
use rskit_fs::sync_io::tree::{WalkControl, WalkOptions, remove_tree_if_exists, walk_tree};

use crate::flags::CacheAction;
use crate::host::Project;

/// Upper bound on cache entries `stats` scans before reporting a truncated
/// count, so an unbounded cache tree cannot exhaust memory or stall the
/// command.
const STATS_ENTRY_CAP: usize = 100_000;

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
///
/// The tree is streamed (not materialized) and the scan stops at
/// [`STATS_ENTRY_CAP`], reporting `truncated=true` so a huge cache cannot
/// exhaust memory.
fn stats(project: &Project) -> AppResult<ExitCode> {
    let root = project.cache_root()?;
    let mut files = 0_usize;
    let mut bytes = 0_u64;
    let mut truncated = false;
    if root.is_dir() {
        walk_tree(&root, WalkOptions::default(), |entry| {
            if !entry.is_file {
                return Ok(WalkControl::Continue);
            }
            if files >= STATS_ENTRY_CAP {
                truncated = true;
                return Ok(WalkControl::Stop);
            }
            files += 1;
            bytes += metadata(&entry.path)?.len;
            Ok(WalkControl::Continue)
        })?;
    }

    let mut summary = OutputKV::new();
    summary
        .add("path", root.display().to_string())
        .add("entries", files.to_string())
        .add("bytes", bytes.to_string())
        .add("truncated", truncated.to_string());
    println!("{summary}");
    Ok(ExitCode::Success)
}

/// Remove the cache directory, reporting whether anything was deleted.
fn clean(project: &Project) -> AppResult<ExitCode> {
    let root = project.cache_root()?;
    let removed = remove_tree_if_exists(&root)?;
    if removed {
        eprintln!("removed cache directory: {}", root.display());
    } else {
        eprintln!("cache directory already absent: {}", root.display());
    }
    Ok(ExitCode::Success)
}

#[cfg(test)]
mod tests {
    use rskit_fs::sync_io::tree::{WalkControl, WalkOptions, walk_tree};

    #[test]
    fn walk_of_an_absent_root_errors_rather_than_silently_counting_zero() {
        let missing = std::env::temp_dir().join("toven-cache-stats-absent-xyz");
        assert!(
            walk_tree(&missing, WalkOptions::default(), |_| Ok(
                WalkControl::Continue
            ))
            .is_err(),
            "walking a missing cache root must surface an error, not count zero"
        );
    }
}
