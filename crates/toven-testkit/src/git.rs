//! Git-scenario helpers built on `rskit-git`.
//!
//! These wrap a `rskit-git` [`Repo`] so vcs-adapter (step 5) and Affected
//! detection (step 6) tests share one git-scenario builder instead of shelling
//! out to raw `git`. The canonical owner of git operations is `rskit-git`; this
//! type only sequences its operations into common test scenarios.

use std::path::{Path, PathBuf};

use rskit_errors::AppResult;
use rskit_fs::sync_io::file;
use rskit_git::{Committer, ConfigReader, Differ, IndexManager, Oid, RefManager, Repo};

/// A git repository under test, with helpers for common scenarios.
///
/// Construct with [`GitScenario::init`] (fresh repo) or [`GitScenario::open`]
/// (existing working tree). All mutating helpers go through `rskit-git`.
pub struct GitScenario {
    repo: Repo,
    root: PathBuf,
}

impl std::fmt::Debug for GitScenario {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitScenario")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl GitScenario {
    /// Initialize a new git repo at `root` with a deterministic test identity.
    pub fn init(root: impl AsRef<Path>) -> AppResult<Self> {
        let root = root.as_ref().to_path_buf();
        let repo = rskit_git::init(&root)?;
        Self::configure_identity(&repo)?;
        Ok(Self { repo, root })
    }

    /// Open an existing git repo at `root`.
    pub fn open(root: impl AsRef<Path>) -> AppResult<Self> {
        let root = root.as_ref().to_path_buf();
        let repo = rskit_git::open(&root)?;
        Ok(Self { repo, root })
    }

    /// Set a deterministic user identity so commits never depend on host config.
    fn configure_identity(repo: &Repo) -> AppResult<()> {
        repo.config_set("user.name", "Toven Test")?;
        repo.config_set("user.email", "test@toven.dev")?;
        Ok(())
    }

    /// The repository working-tree root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Borrow the underlying `rskit-git` [`Repo`].
    #[must_use]
    pub const fn repo(&self) -> &Repo {
        &self.repo
    }

    /// Resolve a path inside the repository working tree.
    fn child(&self, rel: impl AsRef<Path>) -> PathBuf {
        self.root.join(rel)
    }

    /// Write a file (creating parent dirs) inside the working tree.
    pub fn write_file(&self, rel: impl AsRef<Path>, content: &str) -> AppResult<PathBuf> {
        let path = self.child(rel);
        file::create_parent_dir(&path)?;
        file::write(&path, content.as_bytes())?;
        Ok(path)
    }

    /// Stage every change in the working tree.
    pub fn stage_all(&self) -> AppResult<()> {
        let paths = self
            .repo
            .status()?
            .into_iter()
            .map(|entry| entry.path)
            .collect::<Vec<_>>();
        if paths.is_empty() {
            return Ok(());
        }
        let refs = paths.iter().map(String::as_str).collect::<Vec<_>>();
        self.repo.stage(&refs)
    }

    /// Stage all changes and create a commit, returning its object id.
    pub fn commit_all(&self, message: &str) -> AppResult<Oid> {
        self.stage_all()?;
        self.repo.commit(message, None)
    }

    /// Write a file, stage it, and commit — the common "one change" scenario.
    pub fn commit_file(
        &self,
        rel: impl AsRef<Path>,
        content: &str,
        message: &str,
    ) -> AppResult<Oid> {
        self.write_file(rel, content)?;
        self.commit_all(message)
    }

    /// Create a branch at the current `HEAD`.
    pub fn branch(&self, name: &str) -> AppResult<()> {
        self.repo.create_branch(name, "HEAD")
    }

    /// Create an annotated tag at the current `HEAD`.
    pub fn tag(&self, name: &str, message: &str) -> AppResult<()> {
        self.repo.create_tag(name, "HEAD", message)
    }

    /// Short helper: assert a tag exists by name.
    pub fn has_tag(&self, name: &str) -> AppResult<bool> {
        Ok(self.repo.list_tags()?.iter().any(|tag| tag.name == name))
    }

    /// Resolve a reference (e.g. `"HEAD"`, a branch, or a tag) to its hex id.
    pub fn resolve(&self, refname: &str) -> AppResult<String> {
        use rskit_git::Repository;
        self.repo.resolve_ref(refname).map(|oid| oid.to_string())
    }
}
