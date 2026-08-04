//! Shared VCS port doubles: [`FakeVcsReader`] (scripted reads) and
//! [`FakeVcsWriter`] (recording writes).
//!
//! Affected/release tests script `changed_since`, tags, and status here instead
//! of materializing a temp git repo. When a test needs a *real* repo (e.g. to
//! exercise the rskit-git-backed adapter), use
//! [`GitScenario`](crate::git::GitScenario) /
//! [`SampleRepo`](crate::repo::SampleRepo) instead.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_ports::{
    BaselineSpec, ChangeRecord, CommitSummary, Oid, TagRef, TagSigner, VcsReader, VcsWriter,
};

/// A [`VcsReader`] that returns scripted, repo-relative responses.
///
/// All fields default to empty / `"0000000"`; set them with the `with_*`
/// builders. `changed_since` returns the scripted records regardless of the
/// baseline spec — baseline *policy* is the engine's job, not the port's.
#[derive(Debug, Clone)]
pub struct FakeVcsReader {
    branch: Option<String>,
    rev_parse_oid: Oid,
    merge_base_oid: Oid,
    tags: Vec<TagRef>,
    changed: Vec<ChangeRecord>,
    commits: Vec<CommitSummary>,
    worktree: Vec<ChangeRecord>,
    ignored: Vec<PathBuf>,
}

impl Default for FakeVcsReader {
    fn default() -> Self {
        Self {
            branch: Some("main".to_string()),
            rev_parse_oid: Oid::new("0000000"),
            merge_base_oid: Oid::new("0000000"),
            tags: Vec::new(),
            changed: Vec::new(),
            commits: Vec::new(),
            worktree: Vec::new(),
            ignored: Vec::new(),
        }
    }
}

impl FakeVcsReader {
    /// Construct a reader with empty scripted responses.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Script the checked-out local branch returned by `current_branch`.
    #[must_use]
    pub fn with_current_branch(mut self, branch: impl Into<String>) -> Self {
        self.branch = Some(branch.into());
        self
    }

    /// Script `current_branch` to fail as it does on a detached HEAD checkout
    /// — the common CI state that branch-independent operations must tolerate.
    #[must_use]
    pub fn with_detached_head(mut self) -> Self {
        self.branch = None;
        self
    }

    /// Script the `rev_parse` result.
    #[must_use]
    pub fn with_rev_parse(mut self, oid: impl Into<String>) -> Self {
        self.rev_parse_oid = Oid::new(oid);
        self
    }

    /// Script the `merge_base` result.
    #[must_use]
    pub fn with_merge_base(mut self, oid: impl Into<String>) -> Self {
        self.merge_base_oid = Oid::new(oid);
        self
    }

    /// Script the tags returned by `list_tags`.
    #[must_use]
    pub fn with_tags(mut self, tags: Vec<TagRef>) -> Self {
        self.tags = tags;
        self
    }

    /// Script the committed changes returned by `changed_since`.
    #[must_use]
    pub fn with_changed_since(mut self, changes: Vec<ChangeRecord>) -> Self {
        self.changed = changes;
        self
    }

    /// Script the commits returned by `commits_since` (newest first). Returned
    /// regardless of the `since`/`path_prefix` arguments — range and path
    /// scoping are the adapter's job, not the double's.
    #[must_use]
    pub fn with_commits_since(mut self, commits: Vec<CommitSummary>) -> Self {
        self.commits = commits;
        self
    }

    /// Script the working-tree changes returned by `worktree_status`.
    #[must_use]
    pub fn with_worktree_status(mut self, changes: Vec<ChangeRecord>) -> Self {
        self.worktree = changes;
        self
    }

    /// Script the paths reported as git-ignored.
    #[must_use]
    pub fn with_ignored(mut self, paths: Vec<PathBuf>) -> Self {
        self.ignored = paths;
        self
    }
}

impl VcsReader for FakeVcsReader {
    fn current_branch(&self) -> AppResult<String> {
        self.branch.clone().ok_or_else(|| {
            AppError::invalid_input(
                "git.head",
                "HEAD is detached; a configured release branch requires a checked-out local branch",
            )
        })
    }

    fn rev_parse(&self, _rev: &str) -> AppResult<Oid> {
        Ok(self.rev_parse_oid.clone())
    }

    fn merge_base(&self, _a: &str, _b: &str) -> AppResult<Oid> {
        Ok(self.merge_base_oid.clone())
    }

    fn list_tags(&self, _pattern: Option<&str>) -> AppResult<Vec<TagRef>> {
        Ok(self.tags.clone())
    }

    fn changed_since(&self, _spec: &BaselineSpec) -> AppResult<Vec<ChangeRecord>> {
        Ok(self.changed.clone())
    }

    fn commits_since(
        &self,
        _since: Option<&str>,
        _path_prefix: Option<&Path>,
    ) -> AppResult<Vec<CommitSummary>> {
        Ok(self.commits.clone())
    }

    fn worktree_status(&self) -> AppResult<Vec<ChangeRecord>> {
        Ok(self.worktree.clone())
    }

    fn is_ignored(&self, repo_relative: &Path) -> AppResult<bool> {
        Ok(self.ignored.iter().any(|p| p == repo_relative))
    }
}

/// A single recorded write performed against a [`FakeVcsWriter`].
#[derive(Debug, Clone, Eq, PartialEq)]
#[non_exhaustive]
pub enum VcsWrite {
    /// A `commit` call with its message and exact staged paths.
    Commit {
        /// Commit message.
        message: String,
        /// Repo-relative paths supplied to the write port.
        paths: Vec<String>,
    },
    /// A `stage` call with its exact staged paths (PR-first `bump --no-commit`).
    Stage {
        /// Repo-relative paths supplied to the write port.
        paths: Vec<String>,
    },
    /// A `create_tag` call with name, target rev, optional message, and the
    /// signing material when the tag was requested signed.
    CreateTag {
        /// Tag name.
        name: String,
        /// Revision the tag points at.
        target_rev: String,
        /// Annotation message (`None` for a lightweight tag).
        message: Option<String>,
        /// Signing material when the tag was requested signed; `None` for an
        /// unsigned tag.
        signer: Option<TagSigner>,
    },
    /// A `push` call with its remote and refspecs.
    Push {
        /// Remote receiving the push.
        remote: String,
        /// Refspecs supplied to the push.
        refspecs: Vec<String>,
    },
    /// A `restore_worktree` call.
    RestoreWorktree,
}

/// A [`VcsWriter`] that records every history-mutating call for assertions.
///
/// Interior mutability ([`Mutex`]) keeps it `&self`-callable and `Send + Sync`
/// behind `dyn VcsWriter`. Inspect the recorded calls with
/// [`FakeVcsWriter::writes`].
#[derive(Debug)]
pub struct FakeVcsWriter {
    commit_oid: Oid,
    fail_preflight_tag_signer: Option<String>,
    fail_commit: Option<String>,
    fail_stage: Option<String>,
    fail_create_tag: Option<String>,
    fail_push: Option<String>,
    fail_restore: Option<String>,
    writes: Mutex<Vec<VcsWrite>>,
}

impl Default for FakeVcsWriter {
    fn default() -> Self {
        Self {
            commit_oid: Oid::new("0000000"),
            fail_preflight_tag_signer: None,
            fail_commit: None,
            fail_stage: None,
            fail_create_tag: None,
            fail_push: None,
            fail_restore: None,
            writes: Mutex::new(Vec::new()),
        }
    }
}

impl FakeVcsWriter {
    /// Construct a writer that records calls and returns a default commit oid.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Script the object id returned by `commit`.
    #[must_use]
    pub fn with_commit_oid(mut self, oid: impl Into<String>) -> Self {
        self.commit_oid = Oid::new(oid);
        self
    }

    /// Make signed-tag preflight fail with a typed invalid-input error without
    /// recording a history-mutating write.
    #[must_use]
    pub fn with_tag_signer_preflight_failure(mut self, message: impl Into<String>) -> Self {
        self.fail_preflight_tag_signer = Some(message.into());
        self
    }

    /// Make `commit` fail with a typed internal error after recording the call.
    #[must_use]
    pub fn with_commit_failure(mut self, message: impl Into<String>) -> Self {
        self.fail_commit = Some(message.into());
        self
    }

    /// Make `stage` fail with a typed internal error after recording the call —
    /// e.g. to model a PR-first `bump --no-commit` staging failure.
    #[must_use]
    pub fn with_stage_failure(mut self, message: impl Into<String>) -> Self {
        self.fail_stage = Some(message.into());
        self
    }

    /// Make `create_tag` fail with a typed internal error after recording the
    /// call — e.g. to model a post-commit tagging failure.
    #[must_use]
    pub fn with_create_tag_failure(mut self, message: impl Into<String>) -> Self {
        self.fail_create_tag = Some(message.into());
        self
    }

    /// Make `push` fail with a typed internal error after recording the call —
    /// e.g. to model a post-commit push failure.
    #[must_use]
    pub fn with_push_failure(mut self, message: impl Into<String>) -> Self {
        self.fail_push = Some(message.into());
        self
    }

    /// Make `restore_worktree` fail with a typed internal error after recording
    /// the call.
    #[must_use]
    pub fn with_restore_failure(mut self, message: impl Into<String>) -> Self {
        self.fail_restore = Some(message.into());
        self
    }

    /// A snapshot of the recorded writes, in call order.
    #[must_use]
    pub fn writes(&self) -> Vec<VcsWrite> {
        self.writes
            .lock()
            .expect("FakeVcsWriter mutex poisoned")
            .clone()
    }

    fn record(&self, write: VcsWrite) {
        self.writes
            .lock()
            .expect("FakeVcsWriter mutex poisoned")
            .push(write);
    }
}

impl VcsWriter for FakeVcsWriter {
    fn commit(&self, message: &str, paths: &[&str]) -> AppResult<Oid> {
        self.record(VcsWrite::Commit {
            message: message.to_string(),
            paths: paths.iter().map(|path| (*path).to_string()).collect(),
        });
        if let Some(message) = &self.fail_commit {
            return Err(AppError::new(ErrorCode::Internal, message.clone()));
        }
        Ok(self.commit_oid.clone())
    }

    fn stage(&self, paths: &[&str]) -> AppResult<()> {
        self.record(VcsWrite::Stage {
            paths: paths.iter().map(|path| (*path).to_string()).collect(),
        });
        if let Some(message) = &self.fail_stage {
            return Err(AppError::new(ErrorCode::Internal, message.clone()));
        }
        Ok(())
    }

    fn preflight_tag_signer(&self, _signer: &TagSigner) -> AppResult<()> {
        if let Some(message) = &self.fail_preflight_tag_signer {
            return Err(AppError::invalid_input("git.signing_key", message.clone()));
        }
        Ok(())
    }

    fn create_tag(
        &self,
        name: &str,
        target_rev: &str,
        message: Option<&str>,
        signer: Option<&TagSigner>,
    ) -> AppResult<()> {
        self.record(VcsWrite::CreateTag {
            name: name.to_string(),
            target_rev: target_rev.to_string(),
            message: message.map(ToString::to_string),
            signer: signer.cloned(),
        });
        if let Some(message) = &self.fail_create_tag {
            return Err(AppError::new(ErrorCode::Internal, message.clone()));
        }
        Ok(())
    }

    fn push(&self, remote: &str, refspecs: &[String]) -> AppResult<()> {
        self.record(VcsWrite::Push {
            remote: remote.to_string(),
            refspecs: refspecs.to_vec(),
        });
        if let Some(message) = &self.fail_push {
            return Err(AppError::new(ErrorCode::Internal, message.clone()));
        }
        Ok(())
    }

    fn restore_worktree(&self) -> AppResult<()> {
        self.record(VcsWrite::RestoreWorktree);
        if let Some(message) = &self.fail_restore {
            return Err(AppError::new(ErrorCode::Internal, message.clone()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use toven_ports::{
        BaselineSpec, ChangeRecord, ChangeStatus, Oid, TagRef, VcsReader, VcsWriter,
    };

    use super::{FakeVcsReader, FakeVcsWriter, VcsWrite};

    #[test]
    fn reader_returns_scripted_changes() {
        let reader = FakeVcsReader::new()
            .with_changed_since(vec![ChangeRecord::new(
                "src/lib.rs",
                ChangeStatus::Modified,
            )])
            .with_ignored(vec![PathBuf::from("target")]);

        let changed = reader
            .changed_since(&BaselineSpec::explicit("main"))
            .expect("changed");
        assert_eq!(changed.len(), 1);
        assert!(reader.is_ignored(Path::new("target")).expect("ignored"));
        assert!(!reader.is_ignored(Path::new("src")).expect("not ignored"));
    }

    #[test]
    fn reader_returns_scripted_tags() {
        let reader =
            FakeVcsReader::new().with_tags(vec![TagRef::new("errors@1.0.0", Oid::new("cafe"))]);
        assert_eq!(reader.list_tags(None).expect("tags").len(), 1);
    }

    #[test]
    fn writer_records_calls_in_order() {
        let writer = FakeVcsWriter::new().with_commit_oid("abc123");

        let oid = writer.commit("release", &["a.rs"]).expect("commit");
        writer
            .create_tag("v1", "HEAD", Some("rel"), None)
            .expect("tag");
        writer
            .push("origin", &["refs/tags/v1".into()])
            .expect("push");

        assert_eq!(oid.as_str(), "abc123");
        assert_eq!(
            writer.writes(),
            vec![
                VcsWrite::Commit {
                    message: "release".into(),
                    paths: vec!["a.rs".into()],
                },
                VcsWrite::CreateTag {
                    name: "v1".into(),
                    target_rev: "HEAD".into(),
                    message: Some("rel".into()),
                    signer: None,
                },
                VcsWrite::Push {
                    remote: "origin".into(),
                    refspecs: vec!["refs/tags/v1".into()],
                },
            ]
        );
    }
}
