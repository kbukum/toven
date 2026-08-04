//! The history-mutating git seam, scoped to release APPLY.

use rskit_errors::AppResult;

use super::{Oid, TagSigner};

/// The write side of the git port — used **only** by release APPLY.
///
/// Kept separate from [`VcsReader`](super::VcsReader) so read-only callers
/// never carry history-mutating capability. Object-safe; one rskit-git-backed
/// adapter implements both halves.
pub trait VcsWriter {
    /// Create the single release commit from exactly the repo-relative `paths`
    /// the release mutated, and return its object id.
    ///
    /// The paths are the manifests the release's mutations rewrote (empty is
    /// never passed — a mutation-free release tags `HEAD` instead of committing).
    /// The adapter stages precisely those paths, then commits, so the commit
    /// carries the version bump and no unrelated working-tree change leaks in.
    /// The clean-tree guard runs before any mutation, so `paths` are the
    /// release's own writes.
    fn commit(&self, message: &str, paths: &[&str]) -> AppResult<Oid>;

    /// Stage exactly the repo-relative `paths` the release mutated without
    /// creating a commit.
    ///
    /// The PR-first `bump` phase (`--no-commit`) uses this to leave the version
    /// and changelog mutation staged in the index for a maintainer to review and
    /// commit into their own pull request, instead of cutting the release commit
    /// itself. Like [`commit`](Self::commit), the paths are precisely the
    /// release's own writes, so no unrelated working-tree change is staged.
    fn stage(&self, paths: &[&str]) -> AppResult<()>;

    /// Validate that `signer` can create a signed tag without mutating history.
    ///
    /// The release engine calls this before manifest mutation and the release
    /// commit boundary, so a missing inherited signing key fails closed before
    /// any release state exists. Implementations validate local signer
    /// requirements only; the actual signing operation still happens in
    /// [`create_tag`](Self::create_tag).
    fn preflight_tag_signer(&self, signer: &TagSigner) -> AppResult<()>;

    /// Create a tag at `target_rev`. `Some(message)` makes an annotated tag;
    /// `None` makes a lightweight tag. `Some(signer)` makes a signed tag —
    /// signing is always annotated, so `message` must be `Some`; the adapter
    /// returns a typed error otherwise, and preflights that a signing key is
    /// available. `None` signer makes an unsigned tag.
    fn create_tag(
        &self,
        name: &str,
        target_rev: &str,
        message: Option<&str>,
        signer: Option<&TagSigner>,
    ) -> AppResult<()>;

    /// Push the given refspecs (commit + tags) to `remote`.
    fn push(&self, remote: &str, refspecs: &[String]) -> AppResult<()>;

    /// Roll the working tree back to `HEAD` (pre-commit failure undo).
    fn restore_worktree(&self) -> AppResult<()>;
}
