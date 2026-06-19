//! Shared VCS port doubles: [`FakeVcsReader`] (scripted reads) and
//! [`FakeVcsWriter`] (recording writes).
//!
//! Affected/release tests script `changed_since`, tags, and status here instead
//! of materializing a temp git repo. When a test needs a *real* repo (e.g. to
//! exercise the rskit-git-backed adapter), use
//! [`GitScenario`](crate::git::GitScenario) / [`SampleRepo`](crate::repo::SampleRepo)
//! instead.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rskit_errors::AppResult;
use toven_ports::{BaselineSpec, ChangeRecord, Oid, TagRef, VcsReader, VcsWriter};

/// A [`VcsReader`] that returns scripted, repo-relative responses.
///
/// All fields default to empty / `"0000000"`; set them with the `with_*`
/// builders. `changed_since` returns the scripted records regardless of the
/// baseline spec — baseline *policy* is the engine's job, not the port's.
#[derive(Debug, Clone)]
pub struct FakeVcsReader {
    rev_parse_oid: Oid,
    merge_base_oid: Oid,
    tags: Vec<TagRef>,
    changed: Vec<ChangeRecord>,
    worktree: Vec<ChangeRecord>,
    ignored: Vec<PathBuf>,
}

impl Default for FakeVcsReader {
    fn default() -> Self {
        Self {
            rev_parse_oid: Oid::new("0000000"),
            merge_base_oid: Oid::new("0000000"),
            tags: Vec::new(),
            changed: Vec::new(),
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

    fn worktree_status(&self) -> AppResult<Vec<ChangeRecord>> {
        Ok(self.worktree.clone())
    }

    fn is_ignored(&self, repo_relative: &Path) -> AppResult<bool> {
        Ok(self.ignored.iter().any(|p| p == repo_relative))
    }
}

/// A single recorded write performed against a [`FakeVcsWriter`].
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum VcsWrite {
    /// A `commit` call with its message.
    Commit(String),
    /// A `create_tag` call with name, target rev, and optional message.
    CreateTag {
        /// Tag name.
        name: String,
        /// Revision the tag points at.
        target_rev: String,
        /// Annotation message (`None` for a lightweight tag).
        message: Option<String>,
    },
    /// A `push` call with its refspecs.
    Push(Vec<String>),
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
    writes: Mutex<Vec<VcsWrite>>,
}

impl Default for FakeVcsWriter {
    fn default() -> Self {
        Self {
            commit_oid: Oid::new("0000000"),
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
    fn commit(&self, message: &str) -> AppResult<Oid> {
        self.record(VcsWrite::Commit(message.to_string()));
        Ok(self.commit_oid.clone())
    }

    fn create_tag(&self, name: &str, target_rev: &str, message: Option<&str>) -> AppResult<()> {
        self.record(VcsWrite::CreateTag {
            name: name.to_string(),
            target_rev: target_rev.to_string(),
            message: message.map(ToString::to_string),
        });
        Ok(())
    }

    fn push(&self, refspecs: &[String]) -> AppResult<()> {
        self.record(VcsWrite::Push(refspecs.to_vec()));
        Ok(())
    }

    fn restore_worktree(&self) -> AppResult<()> {
        self.record(VcsWrite::RestoreWorktree);
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

        let oid = writer.commit("release").expect("commit");
        writer.create_tag("v1", "HEAD", Some("rel")).expect("tag");
        writer.push(&["refs/tags/v1".into()]).expect("push");

        assert_eq!(oid.as_str(), "abc123");
        assert_eq!(
            writer.writes(),
            vec![
                VcsWrite::Commit("release".into()),
                VcsWrite::CreateTag {
                    name: "v1".into(),
                    target_rev: "HEAD".into(),
                    message: Some("rel".into()),
                },
                VcsWrite::Push(vec!["refs/tags/v1".into()]),
            ]
        );
    }
}
