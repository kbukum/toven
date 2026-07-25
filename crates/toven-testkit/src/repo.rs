//! [`SampleRepo`] — materialize a `fixtures/repos/<name>` tree into a temp dir
//! the real CLI (and adapters) can plan/apply against.
//!
//! The temp dir is a managed `rskit-testutil` [`TestWorkspace`] (deleted on
//! drop). Copying uses `rskit-fs` `copy_tree` (safe-path rooted); the optional
//! `git init` goes through the [`GitScenario`] helper.

use std::path::{Path, PathBuf};

use rskit_errors::AppResult;
use rskit_fs::sync_io::tree::{CopyTreeOptions, copy_tree};
use rskit_testutil::TestWorkspace;

use crate::fixtures;
use crate::git::GitScenario;

/// A sample Toven-app repo materialized into a temporary directory.
///
/// Holds the managed temp [`TestWorkspace`] so the copied tree lives as long as
/// the `SampleRepo`. Use [`SampleRepo::init_git`] to turn it into a real git
/// repo with an initial import commit.
#[derive(Debug)]
pub struct SampleRepo {
    workspace: TestWorkspace,
    root: PathBuf,
}

impl SampleRepo {
    /// Copy `fixtures/repos/<name>` into a fresh temp workspace, injecting the
    /// shared task profiles.
    ///
    /// Fixture `toven.toml`s include `_profiles/<eco>-tasks.toml` instead of
    /// restating the task grammar; config includes may not traverse above the
    /// config root, so the shared `fixtures/repos/_profiles/` tree is copied
    /// into the materialized repo root.
    ///
    /// Returns an error if the named repo fixture does not exist.
    pub fn materialize(name: &str) -> AppResult<Self> {
        let source = fixtures::repo_path(name)?;
        let workspace = TestWorkspace::new(&format!("sample-repo:{name}"));
        let root = workspace.child("repo")?;
        copy_tree(&source, &root, CopyTreeOptions::default())?;
        let profiles = fixtures::path("repos/_profiles")?;
        if profiles.is_dir() {
            copy_tree(
                &profiles,
                &root.join("_profiles"),
                CopyTreeOptions::default(),
            )?;
        }
        Ok(Self { workspace, root })
    }

    /// The materialized repository root (the directory the CLI runs against).
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The managed temp workspace backing this repo.
    #[must_use]
    pub const fn workspace(&self) -> &TestWorkspace {
        &self.workspace
    }

    /// Resolve a path inside the materialized repo.
    pub fn child(&self, rel: impl AsRef<Path>) -> PathBuf {
        self.root.join(rel)
    }

    /// `git init` the materialized repo and create the initial import commit.
    ///
    /// Returns a [`GitScenario`] for further git scripting (branches, tags,
    /// follow-up commits) in change-detection tests.
    pub fn init_git(&self) -> AppResult<GitScenario> {
        let scenario = GitScenario::init(&self.root)?;
        scenario.commit_all("import sample repo")?;
        Ok(scenario)
    }
}
