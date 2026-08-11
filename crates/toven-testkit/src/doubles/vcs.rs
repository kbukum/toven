//! Shared VCS port doubles: [`FakeVcsReader`] (scripted reads) and
//! [`FakeVcsWriter`] (recording writes).
//!
//! Affected/release tests script `changed_since`, tags, and status here instead
//! of materializing a temp git repo. When a test needs a *real* repo (e.g. to
//! exercise the rskit-git-backed adapter), use
//! [`GitScenario`](crate::git::GitScenario) /
//! [`SampleRepo`](crate::repo::SampleRepo) instead.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_ports::{
    BaselineSpec, ChangeRecord, CommitSummary, Oid, TagRef, TagSigner, VcsReader, VcsWriter,
};

/// A [`VcsReader`] that returns scripted, repo-relative responses.
///
/// All fields default to empty / `"0000000"`; set them with the `with_*`
/// builders. `changed_since` returns the scripted records regardless of the
/// baseline spec — baseline *policy* is the engine's job, not the port's. The
/// working-tree status is held behind a shared handle so a test can mutate it
/// *after* construction — modelling a tree that is clean at the release
/// clean-tree guard but carries edits a mid-mutation `on-resolved` hook then
/// produces (see [`FakeVcsReader::worktree_handle`]).
#[derive(Debug, Clone)]
pub struct FakeVcsReader {
    branch: Option<String>,
    rev_parse_oid: Oid,
    merge_base_oid: Oid,
    tags: Vec<TagRef>,
    changed: Vec<ChangeRecord>,
    changed_between: Vec<ChangeRecord>,
    commits: Vec<CommitSummary>,
    worktree: Arc<Mutex<Vec<ChangeRecord>>>,
    worktree_fault: Arc<Mutex<WorktreeStatusFault>>,
    ignored: Vec<PathBuf>,
    files_at_ref: Vec<((String, PathBuf), Vec<u8>)>,
}

impl Default for FakeVcsReader {
    fn default() -> Self {
        Self {
            branch: Some("main".to_string()),
            rev_parse_oid: Oid::new("0000000"),
            merge_base_oid: Oid::new("0000000"),
            tags: Vec::new(),
            changed: Vec::new(),
            changed_between: Vec::new(),
            commits: Vec::new(),
            worktree: Arc::new(Mutex::new(Vec::new())),
            worktree_fault: Arc::new(Mutex::new(WorktreeStatusFault::None)),
            ignored: Vec::new(),
            files_at_ref: Vec::new(),
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

    /// Script the bytes `file_at_ref` returns for a `(reference, repo-relative
    /// path)` pair. An unscripted pair reads as `None` (absent at that
    /// revision), so a test anchors a module's version at an umbrella tag commit
    /// by scripting only the manifests that existed then.
    #[must_use]
    pub fn with_file_at_ref(
        mut self,
        reference: impl Into<String>,
        path: impl Into<PathBuf>,
        contents: impl Into<Vec<u8>>,
    ) -> Self {
        self.files_at_ref
            .push(((reference.into(), path.into()), contents.into()));
        self
    }

    /// Script the committed changes returned by `changed_since`.
    #[must_use]
    pub fn with_changed_since(mut self, changes: Vec<ChangeRecord>) -> Self {
        self.changed = changes;
        self
    }

    /// Script the committed changes returned by `changed_between`. Returned
    /// regardless of the `from`/`to` arguments — endpoint *resolution* is the
    /// engine's job, not the double's.
    #[must_use]
    pub fn with_changed_between(mut self, changes: Vec<ChangeRecord>) -> Self {
        self.changed_between = changes;
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
    pub fn with_worktree_status(self, changes: Vec<ChangeRecord>) -> Self {
        *self
            .worktree
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = changes;
        self
    }

    /// Make the *first* `worktree_status` call that observes a non-empty tree
    /// fail once with a typed internal error, then recover. Models a status read
    /// that faults after a mid-mutation `on-resolved` hook has already produced
    /// working-tree churn, so the abort path (which reads the tree again to
    /// clean up) still sees a working read.
    #[must_use]
    pub fn with_worktree_status_failure_when_dirty(self, message: impl Into<String>) -> Self {
        *self
            .worktree_fault
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            WorktreeStatusFault::OnceWhenDirty(message.into());
        self
    }

    /// Make the `n`-th `worktree_status` call (1-based) fail once with a typed
    /// internal error, then recover. Models a status read that faults at a
    /// precise point in the flow — e.g. the pre-hook untracked snapshot, taken
    /// while the tree is still empty — so a later cleanup read still succeeds.
    #[must_use]
    pub fn with_worktree_status_failure_on_call(
        self,
        call: usize,
        message: impl Into<String>,
    ) -> Self {
        *self
            .worktree_fault
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = WorktreeStatusFault::OnceOnCall {
            call,
            seen: 0,
            message: message.into(),
        };
        self
    }

    /// A shared handle to the working-tree status, so a test (or a scripted
    /// mid-mutation hook double) can append changes *after* the reader is
    /// constructed and after the clean-tree guard has already observed an empty
    /// tree.
    #[must_use]
    pub fn worktree_handle(&self) -> Arc<Mutex<Vec<ChangeRecord>>> {
        Arc::clone(&self.worktree)
    }

    /// Script the paths reported as git-ignored.
    #[must_use]
    pub fn with_ignored(mut self, paths: Vec<PathBuf>) -> Self {
        self.ignored = paths;
        self
    }
}

/// A one-shot fault the [`FakeVcsReader`] injects into `worktree_status`, so a
/// test can model a status read that faults at a precise point in a release
/// flow and then recovers (letting a subsequent cleanup read succeed).
#[derive(Debug, Clone)]
enum WorktreeStatusFault {
    /// No fault: every read succeeds.
    None,
    /// Fail the first read that observes a non-empty tree, then recover.
    OnceWhenDirty(String),
    /// Fail the `call`-th read (1-based), then recover.
    OnceOnCall {
        /// The 1-based ordinal to fail on.
        call: usize,
        /// How many reads have been observed so far.
        seen: usize,
        /// The failure message.
        message: String,
    },
}

impl WorktreeStatusFault {
    /// Advance the fault for one `worktree_status` call over `status`, returning
    /// the failure message when this call should fault (and disarming so later
    /// reads recover).
    fn fire(&mut self, status: &[ChangeRecord]) -> Option<String> {
        match self {
            Self::None => None,
            Self::OnceWhenDirty(message) => {
                if status.is_empty() {
                    return None;
                }
                let message = message.clone();
                *self = Self::None;
                Some(message)
            }
            Self::OnceOnCall {
                call,
                seen,
                message,
            } => {
                *seen += 1;
                if *seen != *call {
                    return None;
                }
                let message = message.clone();
                *self = Self::None;
                Some(message)
            }
        }
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

    fn changed_between(&self, _from: &str, _to: &str) -> AppResult<Vec<ChangeRecord>> {
        Ok(self.changed_between.clone())
    }

    fn commits_since(
        &self,
        _since: Option<&str>,
        _path_prefix: Option<&Path>,
    ) -> AppResult<Vec<CommitSummary>> {
        Ok(self.commits.clone())
    }

    fn worktree_status(&self) -> AppResult<Vec<ChangeRecord>> {
        let status = self
            .worktree
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let fault = self
            .worktree_fault
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .fire(&status);
        if let Some(message) = fault {
            return Err(AppError::new(ErrorCode::Internal, message));
        }
        Ok(status)
    }

    fn is_ignored(&self, repo_relative: &Path) -> AppResult<bool> {
        Ok(self.ignored.iter().any(|p| p == repo_relative))
    }

    fn file_at_ref(&self, reference: &str, repo_relative: &Path) -> AppResult<Option<Vec<u8>>> {
        Ok(self
            .files_at_ref
            .iter()
            .find(|((r, p), _)| r == reference && p == repo_relative)
            .map(|(_, contents)| contents.clone()))
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
    /// A `stage` call with its exact staged paths (PR-first `bump`).
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
    /// e.g. to model a PR-first `bump` staging failure.
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
