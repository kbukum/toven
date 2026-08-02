//! The read-only git seam.

use std::path::Path;

use rskit_errors::AppResult;

use super::{BaselineSpec, ChangeRecord, CommitSummary, Oid, TagRef};

/// The read-only git port — the ONE git seam shared by task affected-detection,
/// release change-detection, the clean-tree guardrail, and discovery ignore
/// checks.
///
/// Git-only and workspace-agnostic: it returns **repo-relative** records and
/// exposes git primitives; the engine owns baseline policy, workspace-prefix
/// stripping, and committed-∪-worktree composition. Object-safe so the engine
/// depends on `dyn VcsReader` with a single rskit-git-backed adapter behind it.
pub trait VcsReader {
    /// Return the checked-out local branch name.
    ///
    /// Returns an error when `HEAD` is detached and a branch name is required.
    fn current_branch(&self) -> AppResult<String>;

    /// Resolve a revision to its object id.
    fn rev_parse(&self, rev: &str) -> AppResult<Oid>;

    /// Resolve the merge base of two revisions.
    fn merge_base(&self, a: &str, b: &str) -> AppResult<Oid>;

    /// List tags, optionally filtered by a glob pattern (e.g. `"errors@*"`).
    fn list_tags(&self, pattern: Option<&str>) -> AppResult<Vec<TagRef>>;

    /// Committed changes from the baseline to `HEAD`.
    fn changed_since(&self, spec: &BaselineSpec) -> AppResult<Vec<ChangeRecord>>;

    /// Commits reachable from `HEAD` but not from `since` (a prior release ref),
    /// newest first, optionally restricted to those touching `path_prefix`.
    ///
    /// `since = None` walks the full history — a module's first release, which
    /// has no prior tag to diff against. The records carry the split
    /// subject/body and author identity a grouped, attributed changelog needs;
    /// the reader stays forge-agnostic (git data only).
    fn commits_since(
        &self,
        since: Option<&str>,
        path_prefix: Option<&Path>,
    ) -> AppResult<Vec<CommitSummary>>;

    /// Uncommitted working-tree changes (staged + unstaged + untracked).
    fn worktree_status(&self) -> AppResult<Vec<ChangeRecord>>;

    /// Whether a repo-relative path is git-ignored (discovery filter).
    fn is_ignored(&self, repo_relative: &Path) -> AppResult<bool>;
}
