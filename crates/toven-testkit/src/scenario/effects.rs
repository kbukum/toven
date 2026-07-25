use std::path::Path;

use rskit_errors::{AppError, AppResult};
use rskit_fs::safe_join;
use rskit_fs::sync_io::tree::{IgnoreWalkOptions, WalkControl, walk_tree_ignoring};
use rskit_fs::sync_io::{dir, file};
use rskit_testutil::{Golden, GoldenMode, Match};

use crate::git::GitScenario;

use super::model::{Effect, Step};

/// The live surfaces one step's effects are evaluated against.
#[derive(Debug, Clone, Copy)]
pub(super) struct EffectContext<'a> {
    /// The materialized repo root.
    pub repo_root: &'a Path,
    /// The scenario-scoped cache directory.
    pub cache_dir: &'a Path,
    /// The scenario directory (owns effect golden files).
    pub scenario_dir: &'a Path,
    /// Verify or bless effect goldens, in step with stream goldens.
    pub mode: GoldenMode,
}

/// Evaluate every effect of `step`, in order.
///
/// # Errors
///
/// The first failing effect returns a typed [`AppError`] naming the step and
/// the specific effect.
pub(super) fn check(step: &Step, cx: &EffectContext<'_>) -> AppResult<()> {
    for effect in &step.effects {
        check_one(effect, cx).map_err(|err| err.context(format!("step '{}'", step.id)))?;
    }
    Ok(())
}

fn check_one(effect: &Effect, cx: &EffectContext<'_>) -> AppResult<()> {
    match effect {
        Effect::CacheEntries(cmp) => {
            let count = count_files(cx.cache_dir)?;
            if !cmp.matches(count) {
                return Err(AppError::conflict(format!(
                    "cache_entries: expected {cmp}, found {count}"
                )));
            }
        }
        Effect::FileExists(rel) => {
            if !path_exists(&repo_path(cx, rel)?)? {
                return Err(AppError::conflict(format!(
                    "file_exists: '{rel}' does not exist in the repo"
                )));
            }
        }
        Effect::PathAbsent(rel) => {
            if path_exists(&repo_path(cx, rel)?)? {
                return Err(AppError::conflict(format!(
                    "path_absent: '{rel}' exists in the repo"
                )));
            }
        }
        Effect::FileMatches { path, golden } => {
            let actual = file::read_string(&repo_path(cx, path)?)
                .map_err(|err| err.context(format!("file_matches: '{path}'")))?;
            // The loader validates `golden` as a bare filename; the safe join
            // keeps bless-mode writes confined even so.
            let golden_path = safe_join(cx.scenario_dir, golden)
                .map_err(|err| AppError::invalid_input("effect golden", err.to_string()))?;
            Golden::new(golden_path, Match::Exact)
                .run(&actual, cx.mode)
                .map_err(|err| err.context(format!("file_matches: '{path}' vs '{golden}'")))?;
        }
        Effect::GitTagExists(tag) => {
            let git = GitScenario::open(cx.repo_root)?;
            if !git.has_tag(tag)? {
                return Err(AppError::conflict(format!(
                    "git_tag_exists: tag '{tag}' does not exist"
                )));
            }
        }
    }
    Ok(())
}

/// Resolve an effect's repo-relative path, rejecting traversal.
fn repo_path(cx: &EffectContext<'_>, rel: &str) -> AppResult<std::path::PathBuf> {
    safe_join(cx.repo_root, rel)
        .map_err(|err| AppError::invalid_input("effect path", err.to_string()))
}

/// Whether any filesystem entry (file, directory, or symlink) exists at
/// `path` — typed: an I/O failure (e.g. permission denied) is an error, never
/// a silent pass in either direction.
fn path_exists(path: &Path) -> AppResult<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(AppError::internal(err)
            .context(format!("failed to stat effect path {}", path.display()))),
    }
}

/// Count regular files under `root` recursively; a missing directory counts
/// as zero entries (a cold cache).
fn count_files(root: &Path) -> AppResult<u64> {
    /// Cache layouts are plain trees: no ignore semantics, hidden files count.
    const WALK: IgnoreWalkOptions = IgnoreWalkOptions {
        respect_gitignore: false,
        skip_hidden: false,
        follow_symlinks: false,
    };
    if !dir::exists(root)? {
        return Ok(0);
    }
    let mut count = 0_u64;
    walk_tree_ignoring(root, WALK, |_entry| {
        count += 1;
        Ok(WalkControl::Continue)
    })?;
    Ok(count)
}
