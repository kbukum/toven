//! Working-tree composition: uncommitted status and the pre-commit
//! [`restore_worktree`] undo.
//!
//! `worktree_status` projects [`Differ::status`](rskit_git::Differ) (staged +
//! unstaged + untracked) onto repo-relative [`ChangeRecord`]s — path-centric, for
//! the affected `committed ∪ worktree` union and the clean-tree guardrail.
//! `restore_worktree` composes [`Resetter::reset`](rskit_git::Resetter) +
//! [`CheckoutManager::checkout_files`](rskit_git::CheckoutManager) to roll tracked
//! files back to `HEAD` when a release apply fails before the commit.

use std::path::PathBuf;

use rskit_errors::AppResult;
use rskit_git::{CheckoutManager, Differ, EntryState, Repo, ResetMode, Resetter, StatusEntry};
use toven_ports::{ChangeRecord, ChangeStatus};

/// Uncommitted working-tree changes (staged + unstaged + untracked), repo-relative.
pub(super) fn worktree_status(repo: &Repo) -> AppResult<Vec<ChangeRecord>> {
    Ok(repo.status()?.into_iter().map(record_from_status).collect())
}

/// Project a working-tree [`StatusEntry`] onto a [`ChangeRecord`].
///
/// The status port reports presence/state, not add-vs-modify granularity, so an
/// untracked entry is `Added` and everything else is `Modified` — sufficient for
/// the path-centric affected union.
fn record_from_status(entry: StatusEntry) -> ChangeRecord {
    let status = match entry.state {
        EntryState::Untracked => ChangeStatus::Added,
        _ => ChangeStatus::Modified,
    };
    ChangeRecord::new(PathBuf::from(entry.path), status)
}

/// Roll tracked working-tree files back to `HEAD` (pre-commit rollback).
///
/// Mixed-resets the index to `HEAD` (unstaging the release writes) then checks
/// the dirty paths back out from the index, restoring their `HEAD` contents.
/// Scoped to files that exist at `HEAD`: release mutations rewrite tracked,
/// already-committed manifests (vcs-port Decision 5), so this is the complete
/// undo for that case. Untracked files — and any first-time, not-yet-committed
/// manifest — are intentionally left in place.
pub(super) fn restore_worktree(repo: &Repo) -> AppResult<()> {
    repo.reset("HEAD", ResetMode::Mixed)?;
    // Read status *after* the reset: tracked-committed files now show as dirty
    // and get restored, while any first-time staged file is now untracked and is
    // skipped — `checkout_files` would otherwise fail on a path absent from HEAD.
    let dirty = repo
        .status()?
        .into_iter()
        .filter(|entry| !matches!(entry.state, EntryState::Untracked))
        .map(|entry| entry.path)
        .collect::<Vec<_>>();
    let paths = dirty.iter().map(String::as_str).collect::<Vec<_>>();
    repo.checkout_files(&paths)
}

#[cfg(test)]
mod tests {
    use rskit_git::{EntryState, StatusEntry};
    use toven_ports::ChangeStatus;

    use super::record_from_status;

    #[test]
    fn untracked_maps_to_added() {
        let record = record_from_status(StatusEntry {
            path: "new.rs".into(),
            state: EntryState::Untracked,
        });
        assert_eq!(record.status, ChangeStatus::Added);
    }

    #[test]
    fn staged_maps_to_modified() {
        let record = record_from_status(StatusEntry {
            path: "src/lib.rs".into(),
            state: EntryState::Staged,
        });
        assert_eq!(record.status, ChangeStatus::Modified);
    }
}
