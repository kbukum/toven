//! [`RskitGitVcs`] — the single rskit-git-backed adapter implementing both
//! halves of the VCS port.
//!
//! Git-only and repo-relative: primitives ([`rev_parse`](VcsReader::rev_parse),
//! [`merge_base`](VcsReader::merge_base)) delegate straight to rskit-git, while
//! the two composed methods live in the sibling `changed` / `worktree` modules.
//! The engine owns baseline policy, workspace-prefix stripping, and the
//! committed-∪-worktree union; this adapter stays policy-free.

use std::path::{Path, PathBuf};

use rskit_errors::{AppError, AppResult};
use rskit_git::{
    Committer, IgnoreReader, Inspector, LogReader, PushOptions, RefManager, RemoteManager, Repo,
    Repository,
};
use toven_ports::{BaselineSpec, ChangeRecord, Oid, TagRef, VcsReader, VcsWriter};

use super::changed::changed_since;
use super::convert::to_oid;
use super::tags::list_tags;
use super::worktree::{restore_worktree, worktree_status};

/// Default push remote when none is configured on the adapter.
const DEFAULT_REMOTE: &str = "origin";

/// The one rskit-git-backed [`VcsReader`] + [`VcsWriter`] adapter.
///
/// Holds an opened rskit-git [`Repo`] plus the canonical repo root (for the
/// engine's prefix-strip) and the push remote release APPLY targets.
pub struct RskitGitVcs {
    repo: Repo,
    root: PathBuf,
    remote: String,
}

impl std::fmt::Debug for RskitGitVcs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RskitGitVcs")
            .field("root", &self.root)
            .field("remote", &self.remote)
            .finish_non_exhaustive()
    }
}

impl RskitGitVcs {
    /// Open the repository rooted at `path`.
    pub fn open(path: impl AsRef<Path>) -> AppResult<Self> {
        Ok(Self::from_repo(rskit_git::open(path)?))
    }

    /// Discover the repository by walking up from `path`.
    pub fn discover(path: impl AsRef<Path>) -> AppResult<Self> {
        Ok(Self::from_repo(rskit_git::discover(path)?))
    }

    fn from_repo(repo: Repo) -> Self {
        let root = repo.root().to_path_buf();
        Self {
            repo,
            root,
            remote: DEFAULT_REMOTE.to_string(),
        }
    }

    /// Override the push remote (defaults to `origin`).
    #[must_use]
    pub fn with_remote(mut self, remote: impl Into<String>) -> Self {
        self.remote = remote.into();
        self
    }

    /// The canonical repository root the engine strips workspace prefixes from.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Whether the working tree has uncommitted changes (clean-tree guardrail).
    pub fn is_dirty(&self) -> AppResult<bool> {
        self.repo.is_dirty()
    }
}

impl VcsReader for RskitGitVcs {
    fn rev_parse(&self, rev: &str) -> AppResult<Oid> {
        self.repo.rev_parse(rev).map(|oid| to_oid(&oid))
    }

    fn merge_base(&self, a: &str, b: &str) -> AppResult<Oid> {
        self.repo.merge_base(a, b).map(|oid| to_oid(&oid))
    }

    fn list_tags(&self, pattern: Option<&str>) -> AppResult<Vec<TagRef>> {
        list_tags(&self.repo, pattern)
    }

    fn changed_since(&self, spec: &BaselineSpec) -> AppResult<Vec<ChangeRecord>> {
        changed_since(&self.repo, spec)
    }

    fn worktree_status(&self) -> AppResult<Vec<ChangeRecord>> {
        worktree_status(&self.repo)
    }

    fn is_ignored(&self, repo_relative: &Path) -> AppResult<bool> {
        let path = repo_relative.to_str().ok_or_else(|| {
            AppError::invalid_input(
                "path",
                format!("non-UTF-8 repo path '{}'", repo_relative.display()),
            )
        })?;
        self.repo.is_ignored(path)
    }
}

impl VcsWriter for RskitGitVcs {
    fn commit(&self, message: &str) -> AppResult<Oid> {
        self.repo.commit(message, None).map(|oid| to_oid(&oid))
    }

    fn create_tag(&self, name: &str, target_rev: &str, message: Option<&str>) -> AppResult<()> {
        // Port contract maps straight onto rskit-git: `Some(_)` = annotated
        // (empty message allowed), `None` = lightweight.
        self.repo.create_tag(name, target_rev, message)
    }

    fn push(&self, refspecs: &[String]) -> AppResult<()> {
        let opts = PushOptions {
            refspecs: refspecs.to_vec(),
            ..PushOptions::default()
        };
        self.repo.push(&self.remote, Some(&opts))
    }

    fn restore_worktree(&self) -> AppResult<()> {
        restore_worktree(&self.repo)
    }
}
