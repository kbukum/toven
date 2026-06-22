//! Per-repo dedup + fan-out — the engine-owned seam that opens **one**
//! [`RskitGitVcs`] per distinct repository and maps each repo-relative
//! [`ChangeRecord`] onto the workspaces beneath it.
//!
//! This is the single-repo case the cross-repo step generalizes to N members:
//! resolve each active workspace's canonical repo root, dedup by root, diff once
//! per repo, then strip each workspace's prefix. The prefix-strip
//! ([`rebase_records`]) and the dedup grouping are pure and unit-testable without
//! a git repo; only [`VcsReaderSet::open`] touches the filesystem.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rskit_errors::AppResult;
use rskit_git::{Repository, repo_relative_path};
use toven_model::WorkspaceId;
use toven_ports::{ChangeRecord, ChangeStatus};

use super::adapter::RskitGitVcs;

/// A workspace's placement within its repo: its identity and the repo-relative
/// prefix from the repo root down to the workspace (empty at the repo root).
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct MemberPlacement {
    id: WorkspaceId,
    prefix: PathBuf,
}

impl MemberPlacement {
    /// Construct a placement from a workspace id and its repo-relative prefix.
    #[must_use]
    pub const fn new(id: WorkspaceId, prefix: PathBuf) -> Self {
        Self { id, prefix }
    }

    /// The placed workspace's identity.
    #[must_use]
    pub const fn id(&self) -> &WorkspaceId {
        &self.id
    }

    /// The repo-relative prefix from the repo root to this workspace.
    #[must_use]
    pub fn prefix(&self) -> &Path {
        &self.prefix
    }

    /// Map repo-relative records onto this workspace by stripping its prefix.
    #[must_use]
    pub fn rebase(&self, records: &[ChangeRecord]) -> Vec<ChangeRecord> {
        rebase_records(records, &self.prefix)
    }
}

/// One distinct repository: the opened adapter plus every workspace beneath it.
#[derive(Debug)]
pub struct RepoGroup {
    root: PathBuf,
    vcs: RskitGitVcs,
    members: Vec<MemberPlacement>,
}

impl RepoGroup {
    /// The canonical repository root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The single reader/writer opened for this repo.
    #[must_use]
    pub const fn vcs(&self) -> &RskitGitVcs {
        &self.vcs
    }

    /// The workspaces beneath this repo, in first-seen order.
    #[must_use]
    pub fn members(&self) -> &[MemberPlacement] {
        &self.members
    }
}

/// The deduped set of repos behind the active workspaces — one reader per repo.
#[derive(Debug)]
pub struct VcsReaderSet {
    groups: Vec<RepoGroup>,
}

impl VcsReaderSet {
    /// Resolve each `(workspace, absolute path)` to its canonical repo root,
    /// dedup by root, and open one [`RskitGitVcs`] per distinct repo.
    pub fn open(members: &[(WorkspaceId, PathBuf)]) -> AppResult<Self> {
        let mut resolved = Vec::with_capacity(members.len());
        for (id, abs) in members {
            let root = rskit_git::discover(abs)?.root().to_path_buf();
            let prefix = repo_relative_path(&root, abs)?;
            resolved.push((id.clone(), root, prefix));
        }

        let mut groups = Vec::new();
        for (root, members) in group_by_root(resolved) {
            let vcs = RskitGitVcs::open(&root)?;
            groups.push(RepoGroup { root, vcs, members });
        }
        Ok(Self { groups })
    }

    /// The deduped repo groups, in first-seen order.
    #[must_use]
    pub fn groups(&self) -> &[RepoGroup] {
        &self.groups
    }
}

/// Dedup `(id, repo_root, prefix)` triples into per-repo groups, preserving the
/// first-seen order of both repos and members.
fn group_by_root(
    resolved: Vec<(WorkspaceId, PathBuf, PathBuf)>,
) -> Vec<(PathBuf, Vec<MemberPlacement>)> {
    let mut order: Vec<PathBuf> = Vec::new();
    let mut groups: HashMap<PathBuf, Vec<MemberPlacement>> = HashMap::new();
    for (id, root, prefix) in resolved {
        if !groups.contains_key(&root) {
            order.push(root.clone());
        }
        groups
            .entry(root)
            .or_default()
            .push(MemberPlacement::new(id, prefix));
    }
    order
        .into_iter()
        .filter_map(|root| groups.remove(&root).map(|members| (root, members)))
        .collect()
}

/// Strip a workspace `prefix` off repo-relative `records`, yielding
/// workspace-relative records for those that intersect the workspace.
///
/// A record whose new path is under `prefix` is rebased (and its `old_path` too,
/// when also under `prefix`). A rename *into* the workspace from an outside
/// source has no in-workspace `old_path`, so it is surfaced as an [`Added`] at
/// the new path rather than a `Renamed` record missing its origin. A record
/// whose *old* path alone is under `prefix` is a rename/delete out of the
/// workspace, surfaced as a [`Deleted`] at the old path. An empty `prefix`
/// denotes the repo root and rebases nothing.
///
/// [`Added`]: toven_ports::ChangeStatus::Added
/// [`Deleted`]: toven_ports::ChangeStatus::Deleted
#[must_use]
pub fn rebase_records(records: &[ChangeRecord], prefix: &Path) -> Vec<ChangeRecord> {
    records
        .iter()
        .filter_map(|record| rebase_one(record, prefix))
        .collect()
}

fn rebase_one(record: &ChangeRecord, prefix: &Path) -> Option<ChangeRecord> {
    let new_in = strip(&record.path, prefix);
    let old_in = record
        .old_path
        .as_deref()
        .and_then(|old| strip(old, prefix));

    if let Some(path) = new_in {
        // A rename whose source lies outside the workspace has no in-workspace
        // origin: from this workspace's view the file simply appeared. Surface
        // it as `Added` so we never emit a `Renamed` record without `old_path`.
        if record.status == ChangeStatus::Renamed && old_in.is_none() {
            return Some(ChangeRecord::new(path, ChangeStatus::Added));
        }
        let mut rebased = ChangeRecord::new(path, record.status);
        if let Some(old) = old_in {
            rebased = rebased.with_old_path(old);
        }
        Some(rebased)
    } else {
        // New path is outside the workspace; only an old path under the prefix
        // remains — a rename/delete out of the workspace's view.
        old_in.map(|path| ChangeRecord::new(path, ChangeStatus::Deleted))
    }
}

fn strip(path: &Path, prefix: &Path) -> Option<PathBuf> {
    if prefix.as_os_str().is_empty() {
        return Some(path.to_path_buf());
    }
    path.strip_prefix(prefix).ok().map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use toven_model::WorkspaceId;
    use toven_ports::{ChangeRecord, ChangeStatus};

    use super::{group_by_root, rebase_records};

    fn ws(id: &str) -> WorkspaceId {
        WorkspaceId::new(id).expect("valid workspace id")
    }

    #[test]
    fn dedups_workspaces_sharing_one_repo_root() {
        let root = PathBuf::from("/repo");
        let resolved = vec![
            (ws("web"), root.clone(), PathBuf::from("apps/web")),
            (ws("api"), root.clone(), PathBuf::from("apps/api")),
        ];

        let groups = group_by_root(resolved);

        assert_eq!(groups.len(), 1);
        let (group_root, members) = &groups[0];
        assert_eq!(group_root, &root);
        assert_eq!(members.len(), 2);
        assert_eq!(members[0].id(), &ws("web"));
        assert_eq!(members[1].prefix(), Path::new("apps/api"));
    }

    #[test]
    fn keeps_distinct_repos_in_first_seen_order() {
        let resolved = vec![
            (ws("a"), PathBuf::from("/repo-b"), PathBuf::new()),
            (ws("b"), PathBuf::from("/repo-a"), PathBuf::new()),
        ];

        let groups = group_by_root(resolved);

        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, PathBuf::from("/repo-b"));
        assert_eq!(groups[1].0, PathBuf::from("/repo-a"));
    }

    #[test]
    fn rebase_strips_prefix_and_drops_outsiders() {
        let records = vec![
            ChangeRecord::new("apps/web/src/main.rs", ChangeStatus::Modified),
            ChangeRecord::new("apps/api/src/lib.rs", ChangeStatus::Added),
        ];

        let rebased = rebase_records(&records, Path::new("apps/web"));

        assert_eq!(rebased.len(), 1);
        assert_eq!(rebased[0].path, PathBuf::from("src/main.rs"));
    }

    #[test]
    fn rebase_empty_prefix_keeps_repo_root_records_verbatim() {
        let records = vec![ChangeRecord::new("src/main.rs", ChangeStatus::Modified)];

        let rebased = rebase_records(&records, Path::new(""));

        assert_eq!(rebased, records);
    }

    #[test]
    fn rebase_strips_both_paths_for_in_workspace_rename() {
        let records = vec![
            ChangeRecord::new("apps/web/new.rs", ChangeStatus::Renamed)
                .with_old_path("apps/web/old.rs"),
        ];

        let rebased = rebase_records(&records, Path::new("apps/web"));

        assert_eq!(rebased[0].path, PathBuf::from("new.rs"));
        assert_eq!(rebased[0].old_path, Some(PathBuf::from("old.rs")));
    }

    #[test]
    fn rebase_surfaces_rename_out_as_deletion() {
        let records = vec![
            ChangeRecord::new("apps/api/moved.rs", ChangeStatus::Renamed)
                .with_old_path("apps/web/moved.rs"),
        ];

        let rebased = rebase_records(&records, Path::new("apps/web"));

        assert_eq!(rebased.len(), 1);
        assert_eq!(rebased[0].path, PathBuf::from("moved.rs"));
        assert_eq!(rebased[0].status, ChangeStatus::Deleted);
        assert!(rebased[0].old_path.is_none());
    }

    #[test]
    fn rebase_surfaces_rename_in_as_addition() {
        let records = vec![
            ChangeRecord::new("apps/web/moved.rs", ChangeStatus::Renamed)
                .with_old_path("apps/api/moved.rs"),
        ];

        let rebased = rebase_records(&records, Path::new("apps/web"));

        assert_eq!(rebased.len(), 1);
        assert_eq!(rebased[0].path, PathBuf::from("moved.rs"));
        assert_eq!(rebased[0].status, ChangeStatus::Added);
        assert!(rebased[0].old_path.is_none());
    }
}
