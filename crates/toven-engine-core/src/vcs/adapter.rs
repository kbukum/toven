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

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_git::{
    ChainAuthProvider, Committer, ConfigReader, DefaultAuthProvider, EnvTokenAuthProvider,
    IgnoreReader, IndexManager, Inspector, LogReader, PushOptions, RefManager, RemoteManager, Repo,
    Repository, SignFormat as GitSignFormat, SignOptions,
};
use toven_ports::{
    BaselineSpec, ChangeRecord, CommitSummary, Oid, SignFormat, TagRef, TagSigner, VcsReader,
    VcsWriter,
};

use super::changed::{changed_between, changed_since};
use super::commits::commits_since;
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
    /// This is the authenticated-open constructor: Toven supplies its forge
    /// policy (the token variable names) while the git layer owns the mechanism.
    /// The provider chain falls through to the transport default when none of
    /// the variables are set, so local development is unaffected. An empty
    /// `token_env` is equivalent to [`open`](Self::open).
    pub fn open_with_token_env(path: impl AsRef<Path>, token_env: &[String]) -> AppResult<Self> {
        if token_env.is_empty() {
            return Self::open(path);
        }
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

    /// The opened rskit-git repository, for adapter-internal range composition
    /// tests.
    #[cfg(test)]
    pub(super) const fn repo(&self) -> &Repo {
        &self.repo
    }
}

/// Build the push/fetch auth provider from Toven's configured token
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

    fn changed_between(&self, from: &str, to: &str) -> AppResult<Vec<ChangeRecord>> {
        changed_between(&self.repo, from, to)
    }

    fn commits_since(
        &self,
        since: Option<&str>,
        path_prefix: Option<&Path>,
    ) -> AppResult<Vec<CommitSummary>> {
        commits_since(&self.repo, since, path_prefix)
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
    fn commit(&self, message: &str, paths: &[&str]) -> AppResult<Oid> {
        // Stage exactly the release-mutated manifests, then commit, so the commit
        // carries the version bump instead of writing an empty tree.
        self.repo.stage(paths)?;
        self.repo.commit(message, None).map(|oid| to_oid(&oid))
    }

    fn stage(&self, paths: &[&str]) -> AppResult<()> {
        // Stage exactly the release-mutated paths for a PR-first `bump`
        // run, leaving the maintainer to create the commit.
        self.repo.stage(paths)
    }

    fn preflight_tag_signer(&self, signer: &TagSigner) -> AppResult<()> {
        let opts = to_sign_options(signer)?;
        match opts.key.as_deref() {
            Some(key) if !key.trim().is_empty() => Ok(()),
            Some(_) => Err(signing_key_missing()),
            None => {
                let key = self.repo.config_get("user.signingkey").map_err(|error| {
                    if error.code() == ErrorCode::NotFound {
                        signing_key_missing()
                    } else {
                        AppError::invalid_input(
                            "git.signing_key",
                            format!("failed to read git signing key configuration: {error}"),
                        )
                        .with_cause(error)
                    }
                })?;
                if key.trim().is_empty() {
                    return Err(signing_key_missing());
                }
                Ok(())
            }
        }
    }

    fn create_tag(
        &self,
        name: &str,
        target_rev: &str,
        message: Option<&str>,
        signer: Option<&TagSigner>,
    ) -> AppResult<()> {
        // Port contract maps straight onto rskit-git: `Some(_)` = annotated (empty
        // message allowed), `None` = lightweight. A signed tag is always
        // annotated, so a signing request without a message is a typed error
        // rather than a silently-unsigned tag; rskit-git preflights that a
        // signing key (`user.signingkey`) is configured.
        if let Some(signer) = signer {
            let Some(message) = message else {
                return Err(AppError::invalid_input(
                    "git.tag",
                    format!(
                        "signed tag '{name}' requires an annotated message; set a tag_message \
                         template or disable sign_tags"
                    ),
                ));
            };
            self.preflight_tag_signer(signer)?;
            let opts = to_sign_options(signer)?;
            return self
                .repo
                .create_signed_tag(name, target_rev, message, &opts);
        }
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

fn signing_key_missing() -> AppError {
    AppError::invalid_input(
        "git.signing_key",
        "signed release tags require a non-blank signing key; set signing_key or configure git \
         user.signingkey",
    )
}

/// Map the port's [`TagSigner`] onto rskit-git's [`SignOptions`], translating the
/// signing backend enum and carrying the optional key through unchanged. `None`
/// fields stay `None` so rskit-git inherits the repository's git configuration.
fn to_sign_options(signer: &TagSigner) -> AppResult<SignOptions> {
    let mut opts = SignOptions::default();
    if let Some(format) = signer.format {
        opts.format = Some(match format {
            SignFormat::OpenPgp => GitSignFormat::OpenPgp,
            SignFormat::Ssh => GitSignFormat::Ssh,
            SignFormat::X509 => GitSignFormat::X509,
            _ => {
                return Err(AppError::invalid_input(
                    "release.sign_format",
                    "unsupported release tag signing format",
                ));
            }
        });
    }
    opts.key.clone_from(&signer.key);
    Ok(opts)
}

#[cfg(test)]
mod tests {
    use toven_ports::{TagSigner, VcsReader, VcsWriter};
    use toven_testkit::{TestWorkspace, git::GitScenario};

    use super::RskitGitVcs;

    #[test]
    fn signed_tag_without_a_message_is_a_typed_error() {
        let workspace = TestWorkspace::new("vcs-signed-tag-no-message");
        let scenario = GitScenario::init(workspace.path()).expect("git init");
        scenario
            .commit_file("README.md", "release", "initial")
            .expect("commit");
        let vcs = RskitGitVcs::open(workspace.path()).expect("open");

        let error = vcs
            .create_tag("v1", "HEAD", None, Some(&TagSigner::default()))
            .expect_err("signing requires an annotated message");

        assert_eq!(error.code(), rskit_errors::ErrorCode::InvalidInput);
        assert!(error.message().contains("annotated"), "{error}");
    }

    #[test]
    fn signed_tag_without_a_configured_key_surfaces_the_preflight_error() {
        let workspace = TestWorkspace::new("vcs-signed-tag-no-key");
        let scenario = GitScenario::init(workspace.path()).expect("git init");
        scenario
            .commit_file("README.md", "release", "initial")
            .expect("commit");
        let vcs = RskitGitVcs::open(workspace.path()).expect("open");

        // A fresh scenario configures no signing key, so rskit-git's preflight
        // fails closed with an actionable configuration error.
        let error = vcs
            .preflight_tag_signer(&TagSigner::default())
            .expect_err("no signing key configured");

        assert_eq!(error.code(), rskit_errors::ErrorCode::InvalidInput);
        assert!(error.message().contains("user.signingkey"), "{error}");
    }

    #[test]
    fn signed_tag_without_a_configured_key_surfaces_the_preflight_error_at_tag_creation() {
        let workspace = TestWorkspace::new("vcs-signed-tag-no-key-at-create");
        let scenario = GitScenario::init(workspace.path()).expect("git init");
        scenario
            .commit_file("README.md", "release", "initial")
            .expect("commit");
        let vcs = RskitGitVcs::open(workspace.path()).expect("open");

        let error = vcs
            .create_tag(
                "v1",
                "HEAD",
                Some("release 1.0.0"),
                Some(&TagSigner::default()),
            )
            .expect_err("no signing key configured");

        assert_eq!(error.code(), rskit_errors::ErrorCode::InvalidInput);
        assert!(error.message().contains("user.signingkey"), "{error}");
    }

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

    #[test]
    fn open_with_token_env_is_equivalent_to_open_when_empty() {
        // An empty policy carries no token, so the constructor short-circuits to
        // a plain open and never installs auth callbacks.
        let workspace = TestWorkspace::new("vcs-token-env-empty");
        let scenario = GitScenario::init(workspace.path()).expect("git init");
        scenario
            .commit_file("README.md", "release", "initial")
            .expect("commit");

        let branch = RskitGitVcs::open_with_token_env(workspace.path(), &[])
            .expect("open with empty token env")
            .current_branch()
            .expect("branch");

        assert!(!branch.is_empty());
    }
}
