use std::collections::BTreeSet;

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_ports::{ChangeRecord, ChangeStatus, VcsReader, VcsWriter};

/// Cap on the number of dirty paths named in the clean-tree guard error, so a
/// pathologically dirty tree cannot produce an unbounded message.
const MAX_NAMED_DIRTY_PATHS: usize = 20;

/// Render the offending worktree changes as a bounded, sorted `"status path"`
/// list for the clean-tree guard error, so an operator sees *which* files are
/// dirty (e.g. a CI-only `go.sum`) rather than an opaque count. The list is
/// truncated to [`MAX_NAMED_DIRTY_PATHS`] with a `… and N more` tail.
fn describe_dirty_paths(changes: &[ChangeRecord]) -> String {
    let mut rendered: Vec<String> = changes
        .iter()
        .map(|change| {
            let label = match change.status {
                ChangeStatus::Added => "added",
                ChangeStatus::Modified => "modified",
                ChangeStatus::Deleted => "deleted",
                ChangeStatus::Renamed => "renamed",
            };
            format!("{label} {}", change.path.display())
        })
        .collect();
    rendered.sort();
    if rendered.len() > MAX_NAMED_DIRTY_PATHS {
        let extra = rendered.len() - MAX_NAMED_DIRTY_PATHS;
        rendered.truncate(MAX_NAMED_DIRTY_PATHS);
        rendered.push(format!("… and {extra} more"));
    }
    rendered.join(", ")
}

/// Wrap a failure that happens after externally visible release state exists
/// (`state` says what) with forward-only recovery guidance, preserving the
/// original error code and cause. Past that point the run cannot be made to
/// look atomic: the operator inspects the partially released state and
/// forward-fixes — never rewrites or deletes published state.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn forward_recovery_error(state: &str, phase: &str, error: AppError) -> AppError {
    AppError::new(
        error.code(),
        format!(
            "release {phase} failed after {state}: {error}. Release tags, registry versions, \
             and hosted releases are immutable — inspect `toven release status`, resolve the \
             cause, preview again, and publish a forward fix; never rewrite or delete published \
             state"
        ),
    )
    .with_cause(error)
}

/// Reject a disallowed checked-out branch before release mutation.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn guard_release_branch(
    reader: &dyn VcsReader,
    branches: &BTreeSet<String>,
) -> AppResult<()> {
    if branches.is_empty() {
        return Ok(());
    }
    let branch = reader.current_branch()?;
    if branches.contains(&branch) {
        return Ok(());
    }
    Err(AppError::invalid_input(
        "release.branches",
        format!(
            "checked-out branch '{branch}' is not allowed to cut this release (allowed: {})",
            branches.iter().cloned().collect::<Vec<_>>().join(", ")
        ),
    ))
}

#[allow(clippy::redundant_pub_crate)]
pub(crate) fn restore_or_precommit_error(
    writer: &dyn VcsWriter,
    phase: &str,
    error: AppError,
) -> AppError {
    match writer.restore_worktree() {
        Ok(()) => error,
        Err(restore) => AppError::new(
            ErrorCode::Internal,
            format!(
                "release {phase} failed ({error}); additionally failed to restore worktree: {restore}"
            ),
        )
        .with_cause(error)
        .with_detail("restore_error", restore.to_string()),
    }
}

/// Reject a dirty working tree — the release transaction requires a clean tree.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn guard_clean_tree(reader: &dyn VcsReader) -> AppResult<()> {
    let status = reader.worktree_status()?;
    if status.is_empty() {
        return Ok(());
    }
    Err(AppError::invalid_input(
        "release.worktree",
        format!(
            "the working tree has {} uncommitted change(s); commit or stash them before \
             releasing: {}",
            status.len(),
            describe_dirty_paths(&status)
        ),
    ))
}
