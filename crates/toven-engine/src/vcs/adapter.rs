//! [`RskitGitVcs`] — the single rskit-git-backed adapter implementing both
//! halves of the VCS port.
//!
//! Git-only and repo-relative: primitives ([`rev_parse`](VcsReader::rev_parse),
//! [`merge_base`](VcsReader::merge_base)) delegate straight to rskit-git, while
//! the two composed methods live in the sibling `changed` / `worktree` modules.
//! The engine owns baseline policy, workspace-prefix stripping, and the
//! committed-∪-worktree union; this adapter stays policy-free.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rskit_errors::{AppError, AppResult};
use rskit_git::{
    ChainAuthProvider, Committer, DefaultAuthProvider, EnvTokenAuthProvider, IgnoreReader,
    Inspector, LogReader, PushOptions, RefManager, RemoteManager, Repo, Repository,
};
use toven_ports::{BaselineSpec, ChangeRecord, Oid, TagRef, VcsReader, VcsWriter};

use super::changed::changed_since;
use super::convert::to_oid;
use super::tags::list_tags;
use super::worktree::{restore_worktree, worktree_status};

/// The one rskit-git-backed [`VcsReader`] + [`VcsWriter`] adapter.
///
/// Holds an opened rskit-git [`Repo`] plus the canonical repo root (for the
/// engine's prefix-strip).
pub struct RskitGitVcs {
    repo: Repo,
    root: PathBuf,
}

impl std::fmt::Debug for RskitGitVcs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RskitGitVcs")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl RskitGitVcs {
    /// Open the repository rooted at `path`.
    pub fn open(path: impl AsRef<Path>) -> AppResult<Self> {
        Ok(Self::from_repo(rskit_git::open(path)?))
    }

    /// Open the repository rooted at `path`, authenticating push/fetch with a
    /// token read from the first present variable in `token_env`.
    ///
    /// This is the release-path constructor: Toven supplies its forge policy
    /// (the token variable names) while the git layer owns the mechanism. The
    /// provider chain falls through to the transport default when none of the
    /// variables are set, so local development is unaffected. An empty
    /// `token_env` is equivalent to [`open`](Self::open).
    pub fn open_with_token_env(path: impl AsRef<Path>, token_env: &[String]) -> AppResult<Self> {
        Ok(Self::from_repo(rskit_git::open_with_auth(
            path,
            token_env_auth(token_env),
        )?))
    }

    /// Discover the repository by walking up from `path`.
    pub fn discover(path: impl AsRef<Path>) -> AppResult<Self> {
        Ok(Self::from_repo(rskit_git::discover(path)?))
    }

    fn from_repo(repo: Repo) -> Self {
        let root = repo.root().to_path_buf();
        Self { repo, root }
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

/// Build the release push/fetch auth provider from Toven's configured token
/// variable names: try an env-token first, then fall through to the transport
/// default so an unset token (local development) changes nothing.
fn token_env_auth(token_env: &[String]) -> Arc<dyn rskit_git::AuthProvider> {
    Arc::new(ChainAuthProvider::new(vec![
        Arc::new(EnvTokenAuthProvider::with_vars(token_env.iter().cloned())),
        Arc::new(DefaultAuthProvider),
    ]))
}

impl VcsReader for RskitGitVcs {
    fn current_branch(&self) -> AppResult<String> {
        let head = self.repo.head()?;
        if !head.is_branch {
            return Err(AppError::invalid_input(
                "git.head",
                "HEAD is detached; a configured release branch requires a checked-out local branch",
            ));
        }
        let branch = head.name.strip_prefix("refs/heads/").unwrap_or(&head.name);
        if branch.is_empty() {
            return Err(AppError::invalid_input(
                "git.head",
                "HEAD does not name a local branch",
            ));
        }
        Ok(branch.to_string())
    }

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
        // Port contract maps straight onto rskit-git: `Some(_)` = annotated (empty
        // message allowed), `None` = lightweight.
        self.repo.create_tag(name, target_rev, message)
    }

    fn push(&self, remote: &str, refspecs: &[String]) -> AppResult<()> {
        let opts = PushOptions {
            refspecs: refspecs.to_vec(),
            ..PushOptions::default()
        };
        self.repo.push(remote, Some(&opts))
    }

    fn restore_worktree(&self) -> AppResult<()> {
        restore_worktree(&self.repo)
    }
}

#[cfg(test)]
mod tests {
    use toven_ports::VcsReader;
    use toven_testkit::{TestWorkspace, git::GitScenario};

    use super::RskitGitVcs;

    #[test]
    fn current_branch_returns_the_checked_out_local_branch() {
        let workspace = TestWorkspace::new("vcs-current-branch");
        let scenario = GitScenario::init(workspace.path()).expect("git init");
        scenario
            .commit_file("README.md", "release", "initial")
            .expect("commit");

        let branch = RskitGitVcs::open(workspace.path())
            .expect("open")
            .current_branch()
            .expect("branch");

        assert!(!branch.is_empty());
    }

    #[test]
    fn open_with_token_env_defers_to_transport_default_when_unset() {
        // With no token variable set (the local-development case) the auth chain
        // falls through to the transport default, so opening with a token-env
        // policy behaves exactly like a plain open.
        let workspace = TestWorkspace::new("vcs-token-env-open");
        let scenario = GitScenario::init(workspace.path()).expect("git init");
        scenario
            .commit_file("README.md", "release", "initial")
            .expect("commit");

        let branch = RskitGitVcs::open_with_token_env(
            workspace.path(),
            &["TOVEN_VCS_TEST_ABSENT_TOKEN_7C21".to_string()],
        )
        .expect("open with token env")
        .current_branch()
        .expect("branch");

        assert!(!branch.is_empty());
    }
}
