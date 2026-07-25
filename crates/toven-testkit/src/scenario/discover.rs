use std::path::{Path, PathBuf};

use rskit_errors::{AppError, AppResult};
use rskit_fs::sync_io::dir;
use rskit_fs::sync_io::tree::{IgnoreWalkOptions, WalkControl, walk_tree_ignoring};

use super::load::SCENARIO_FILENAME;

/// Discover every scenario directory under `root`, recursively and sorted.
///
/// A scenario directory is any directory containing a `scenario.yaml`. The
/// sorted order keeps harness reporting deterministic.
///
/// # Errors
///
/// Returns a `NotFound`-class [`AppError`] when `root` itself does not exist —
/// a misconfigured golden root must fail loudly, never pass as an empty suite.
pub fn discover_scenarios(root: &Path) -> AppResult<Vec<PathBuf>> {
    /// Scenario trees are plain data: no ignore semantics, hidden files count.
    const WALK: IgnoreWalkOptions = IgnoreWalkOptions {
        respect_gitignore: false,
        skip_hidden: false,
        follow_symlinks: false,
    };
    if !dir::exists(root)? {
        return Err(AppError::not_found(
            "golden scenario root",
            Some(&root.display().to_string()),
        ));
    }
    let mut dirs = Vec::new();
    walk_tree_ignoring(root, WALK, |entry| {
        if entry
            .path
            .file_name()
            .is_some_and(|name| name == SCENARIO_FILENAME)
            && let Some(parent) = entry.path.parent()
        {
            dirs.push(parent.to_path_buf());
        }
        Ok(WalkControl::Continue)
    })?;
    dirs.sort();
    Ok(dirs)
}
