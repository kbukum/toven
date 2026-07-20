//! The history-mutating git seam, scoped to release APPLY.

use rskit_errors::AppResult;

use super::Oid;

/// The write side of the git port — used **only** by release APPLY.
///
/// Kept separate from [`VcsReader`](super::VcsReader) so read-only callers
/// never carry history-mutating capability. Object-safe; one rskit-git-backed
/// adapter implements both halves.
pub trait VcsWriter {
    /// Create the single release commit; returns its object id.
    fn commit(&self, message: &str) -> AppResult<Oid>;

    /// Create a tag at `target_rev`. `Some(message)` makes an annotated tag;
    /// `None` makes a lightweight tag.
    fn create_tag(&self, name: &str, target_rev: &str, message: Option<&str>) -> AppResult<()>;

    /// Push the given refspecs (commit + tags) to `remote`.
    fn push(&self, remote: &str, refspecs: &[String]) -> AppResult<()>;

    /// Roll the working tree back to `HEAD` (pre-commit failure undo).
    fn restore_worktree(&self) -> AppResult<()>;
}
