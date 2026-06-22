//! `changed_since` — committed `base..HEAD` change detection.
//!
//! Composes two rskit-git primitives the port does *not* expose as one call:
//! resolve the baseline ([`LogReader::merge_base`](rskit_git::LogReader) for
//! `--merge-base`, the reference verbatim otherwise) then
//! [`Differ::diff`](rskit_git::Differ) `base..HEAD`. Records are **repo-relative**.
//!
//! Note on renames: rskit-git's committed tree-to-tree diff does not enable
//! rename detection, so a committed rename currently arrives as a `Deleted` +
//! `Added` pair — which already reproduces the engine's delete/rename
//! double-insert. The `Renamed`/`old_path` mapping below is the faithful
//! projection for the day rename detection is enabled in the backend; it is a
//! no-op for today's diffs. (See the rskit follow-up note in the step handoff.)

use std::path::PathBuf;

use rskit_errors::AppResult;
use rskit_git::{DiffEntry, Differ, FileStatus, LogReader, Repo};
use toven_ports::{BaselineMode, BaselineSpec, ChangeRecord};

use super::convert::map_diff_status;

/// Committed changes from the baseline described by `spec` up to `HEAD`.
pub(super) fn changed_since(repo: &Repo, spec: &BaselineSpec) -> AppResult<Vec<ChangeRecord>> {
    let base = match spec.mode {
        BaselineMode::Explicit => spec.reference.clone(),
        BaselineMode::MergeBase => repo.merge_base(&spec.reference, "HEAD")?.to_string(),
    };
    let diff = repo.diff(&base, "HEAD")?;
    Ok(diff.into_iter().map(record_from_diff).collect())
}

/// Map a single rskit-git [`DiffEntry`] onto a repo-relative [`ChangeRecord`],
/// preserving the pre-rename path so the engine owns the double-insert.
fn record_from_diff(entry: DiffEntry) -> ChangeRecord {
    let record = ChangeRecord::new(PathBuf::from(entry.path), map_diff_status(entry.status));
    match (entry.status, entry.old_path) {
        (FileStatus::Renamed, Some(old_path)) => record.with_old_path(PathBuf::from(old_path)),
        _ => record,
    }
}

#[cfg(test)]
mod tests {
    use rskit_git::{DiffEntry, FileStatus, Oid as GitOid};
    use toven_ports::ChangeStatus;

    use super::record_from_diff;

    fn diff_entry(path: &str, old_path: Option<&str>, status: FileStatus) -> DiffEntry {
        DiffEntry {
            path: path.to_string(),
            old_path: old_path.map(ToString::to_string),
            old_oid: GitOid::from_bytes([0; 20]),
            new_oid: GitOid::from_bytes([0; 20]),
            status,
        }
    }

    #[test]
    fn rename_carries_old_path() {
        let record = record_from_diff(diff_entry("new.rs", Some("old.rs"), FileStatus::Renamed));
        assert_eq!(record.status, ChangeStatus::Renamed);
        assert_eq!(
            record.old_path.as_deref(),
            Some(std::path::Path::new("old.rs"))
        );
    }

    #[test]
    fn modification_has_no_old_path() {
        let record = record_from_diff(diff_entry("src/lib.rs", None, FileStatus::Modified));
        assert_eq!(record.status, ChangeStatus::Modified);
        assert!(record.old_path.is_none());
    }

    #[test]
    fn copy_becomes_added_without_old_path() {
        let record = record_from_diff(diff_entry("copy.rs", Some("src.rs"), FileStatus::Copied));
        assert_eq!(record.status, ChangeStatus::Added);
        assert!(record.old_path.is_none());
    }
}
