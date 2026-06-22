//! Small rskit-git ↔ toven-ports value conversions shared across the adapter
//! compositions.

use rskit_git::{FileStatus, Oid as GitOid};
use toven_ports::{ChangeStatus, Oid};

/// Wrap an rskit-git object id as the ports' opaque [`Oid`] (hex string).
pub(super) fn to_oid(oid: &GitOid) -> Oid {
    Oid::new(oid.to_string())
}

/// Collapse rskit-git's richer [`FileStatus`] onto the ports' four-state
/// [`ChangeStatus`].
///
/// A copy is a new path (`Added`); a type change is a content change
/// (`Modified`). `Untracked` / `Ignored` / `Conflicted` do not appear in a
/// committed `base..HEAD` diff but are mapped conservatively for totality, since
/// [`FileStatus`] is `#[non_exhaustive]`.
pub(super) const fn map_diff_status(status: FileStatus) -> ChangeStatus {
    match status {
        FileStatus::Added | FileStatus::Copied | FileStatus::Untracked => ChangeStatus::Added,
        FileStatus::Deleted => ChangeStatus::Deleted,
        FileStatus::Renamed => ChangeStatus::Renamed,
        _ => ChangeStatus::Modified,
    }
}
