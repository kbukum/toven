use std::collections::BTreeSet;

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_ports::{ChangeRecord, VcsReader, VcsWriter};

/// Cap on the number of dirty paths named in the clean-tree guard error, so a
/// pathologically dirty tree cannot produce an unbounded message.
const MAX_NAMED_DIRTY_PATHS: usize = 20;

/// Total-byte ceiling on the rendered dirty-path list. Worktree paths are
/// repository-controlled and individually unbounded in length, so the
/// per-entry count cap alone cannot bound the message: a single crafted
/// filename could still be arbitrarily long. The named entries are truncated to
/// this many bytes (with an ellipsis) as a hard backstop, with room reserved for
/// the `… and N more` tail so byte truncation can never swallow it.
const MAX_RENDERED_BYTES: usize = 4096;

/// Delimiter between rendered dirty-path entries.
const ENTRY_SEPARATOR: &str = ", ";

/// Render a repository-controlled path as a quoted, control-escaped entry for
/// single-line diagnostic display.
///
/// Worktree paths are untrusted input: a crafted filename can embed newlines,
/// terminal escape sequences, or the [`ENTRY_SEPARATOR`] itself to forge or
/// corrupt the guard error if rendered verbatim. Debug-formatting the path's
/// display string wraps it in quotes and escapes control characters,
/// backslashes, and quotes, so the delimiter and any control byte stay
/// contained within a single entry (a path such as `a, deleted x` renders as
/// the one entry `"a, deleted x"`, not two).
fn quote_path(path: &std::path::Path) -> String {
    format!("{:?}", path.display().to_string())
}

/// Render the offending worktree changes as a bounded, sorted list of quoted
/// paths for the clean-tree guard error, so an operator sees *which* files are
/// dirty (e.g. a CI-only `go.sum`) rather than an opaque count.
///
/// Only the path is rendered, not a status word: the worktree-status source
/// collapses every tracked state to a single value, so a per-entry `added`/
/// `deleted`/`modified` label would be unreliable. The output is bounded twice
/// over untrusted input — to [`MAX_NAMED_DIRTY_PATHS`] entries with a preserved
/// `… and N more` tail, and to [`MAX_RENDERED_BYTES`] total bytes via
/// [`rskit_util::strings::truncate_owned`] — and each path is quoted by
/// [`quote_path`] so a crafted filename cannot forge the diagnostic.
fn describe_dirty_paths(changes: &[ChangeRecord]) -> String {
    let mut rendered: Vec<String> = changes
        .iter()
        .map(|change| quote_path(&change.path))
        .collect();
    rendered.sort();

    let omitted = rendered.len().saturating_sub(MAX_NAMED_DIRTY_PATHS);
    rendered.truncate(MAX_NAMED_DIRTY_PATHS);
    let named = rendered.join(ENTRY_SEPARATOR);

    if omitted == 0 {
        return rskit_util::strings::truncate_owned(&named, MAX_RENDERED_BYTES);
    }

    // Reserve room for the omission tail so the byte cap bounds only the named
    // entries and can never remove the `… and N more` summary.
    let tail = format!("… and {omitted} more");
    let budget = MAX_RENDERED_BYTES.saturating_sub(tail.len() + ENTRY_SEPARATOR.len());
    let named = rskit_util::strings::truncate_owned(&named, budget);
    format!("{named}{ENTRY_SEPARATOR}{tail}")
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
