use std::collections::BTreeSet;

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_ports::{VcsReader, VcsWriter};

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
            "the working tree has {} uncommitted change(s); commit or stash them before releasing",
            status.len()
        ),
    ))
}
