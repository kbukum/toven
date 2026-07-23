//! Git-scenario helpers built on `rskit-git`.
//!
//! These wrap a `rskit-git` [`Repo`] so the VCS adapter and affected-change
//! detection tests share one git-scenario builder instead of shelling out to
//! raw `git`. The canonical owner of git operations is `rskit-git`; this type
//! only sequences its operations into common test scenarios.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rskit_errors::AppResult;
use rskit_fs::sync_io::file;
use rskit_fs::sync_io::tree::{IgnoreWalkOptions, WalkControl, walk_tree_ignoring};
use rskit_git::{
    BranchFilter, Committer, ConfigReader, Differ, IndexManager, Oid, PushOptions, RefManager,
    RemoteManager, Repo,
};
use rskit_util::hash::hash_hex;

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

    /// Set a deterministic user identity so commits never depend on host
    /// config.
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

    /// Check out an existing branch/ref, moving `HEAD` onto it. General-purpose
    /// helper for any scenario that must run on a specific named branch.
    pub fn checkout(&self, refname: &str) -> AppResult<()> {
        use rskit_git::CheckoutManager;
        self.repo.checkout(refname, None)
    }

    /// Create a branch at `HEAD` and immediately check it out.
    pub fn branch_and_checkout(&self, name: &str) -> AppResult<()> {
        self.branch(name)?;
        self.checkout(name)
    }

    /// Create an annotated tag at the current `HEAD`.
    pub fn tag(&self, name: &str, message: &str) -> AppResult<()> {
        self.repo.create_tag(name, "HEAD", Some(message))
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

    /// Initialize a bare repository at `root` — a general-purpose local remote
    /// usable as a push/fetch target by any scenario (release, sync, mirror).
    ///
    /// Returns the bare repository path so callers can wire it as a remote URL
    /// and later snapshot its refs with [`ref_map_at`].
    pub fn init_bare(root: impl AsRef<Path>) -> AppResult<PathBuf> {
        let root = root.as_ref().to_path_buf();
        rskit_git::init_bare(&root)?;
        Ok(root)
    }

    /// Configure a named remote pointing at `url` (a local bare-repo path or
    /// URL). General-purpose: any test needing a push/fetch target uses this.
    pub fn add_remote(&self, name: &str, url: impl AsRef<Path>) -> AppResult<()> {
        let url = url.as_ref();
        let url = url.to_str().ok_or_else(|| {
            rskit_errors::AppError::invalid_input("remote.url", "non-UTF-8 remote path")
        })?;
        self.repo.config_set(&format!("remote.{name}.url"), url)?;
        self.repo.config_set(
            &format!("remote.{name}.fetch"),
            &format!("+refs/heads/*:refs/remotes/{name}/*"),
        )?;
        Ok(())
    }

    /// Push explicit refspecs to a configured remote (thin pass-through to the
    /// rskit-git owner). General-purpose for any push scenario.
    pub fn push(&self, remote: &str, refspecs: &[String]) -> AppResult<()> {
        let opts = PushOptions {
            refspecs: refspecs.to_vec(),
            ..PushOptions::default()
        };
        self.repo.push(remote, Some(&opts))
    }

    /// Snapshot every local branch and tag ref to `refname -> hex-oid`.
    ///
    /// A general-purpose ref-map for before/after diffing in any scenario
    /// (release push correctness, mutation-free previews, sync, mirror). Keys
    /// are fully qualified (`refs/heads/<name>`, `refs/tags/<name>`).
    pub fn ref_map(&self) -> AppResult<BTreeMap<String, String>> {
        ref_map_of(&self.repo)
    }
}

/// Snapshot every local branch and tag ref of the repository at `path`.
///
/// Opens the repository (working-tree or bare) and returns a general-purpose
/// `refname -> hex-oid` map — the primitive for asserting what a bare remote
/// received after a push, independent of any single feature.
pub fn ref_map_at(path: impl AsRef<Path>) -> AppResult<BTreeMap<String, String>> {
    let repo = rskit_git::open(path)?;
    ref_map_of(&repo)
}

/// Snapshot every visible, non-ignored working-tree file to `relpath ->
/// content-digest`.
///
/// A general-purpose worktree digest for before/after diffing in any scenario
/// (mutation-free previews, idempotent reruns, sync, mirror). Honours
/// `.gitignore` and skips dot-prefixed entries — notably the `.git` database —
/// so build artifacts and git internals never perturb the comparison. Keys are
/// forward-slash relative paths for cross-platform stability.
pub fn worktree_digests(root: impl AsRef<Path>) -> AppResult<BTreeMap<String, String>> {
    /// Per-file read cap; a test worktree file larger than this is unexpected.
    const MAX_DIGEST_BYTES: u64 = 16 * 1024 * 1024;
    /// Honour ignore files and skip hidden entries (the `.git` DB included).
    const WALK: IgnoreWalkOptions = IgnoreWalkOptions {
        respect_gitignore: true,
        skip_hidden: true,
        follow_symlinks: false,
    };

    let mut digests = BTreeMap::new();
    walk_tree_ignoring(root.as_ref(), WALK, |entry| {
        let bytes = file::read_bounded(&entry.path, MAX_DIGEST_BYTES)?;
        digests.insert(
            entry.relative_path.to_string_lossy().replace('\\', "/"),
            hash_hex(&bytes),
        );
        Ok(WalkControl::Continue)
    })?;
    Ok(digests)
}

/// Shared ref-map snapshot over an opened [`Repo`].
fn ref_map_of(repo: &Repo) -> AppResult<BTreeMap<String, String>> {
    let mut map = BTreeMap::new();
    for branch in repo.list_branches(BranchFilter::Local)? {
        map.insert(
            format!("refs/heads/{}", branch.name),
            branch.target.to_string(),
        );
    }
    for tag in repo.list_tags()? {
        map.insert(format!("refs/tags/{}", tag.name), tag.target.to_string());
    }
    Ok(map)
}
