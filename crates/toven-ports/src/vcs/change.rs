//! Repo-relative change records returned by [`VcsReader`](super::VcsReader).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The nature of a single path change.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChangeStatus {
    /// The path was added.
    Added,
    /// The path's contents changed.
    Modified,
    /// The path was deleted.
    Deleted,
    /// The path was renamed (see [`ChangeRecord::old_path`]).
    Renamed,
}

/// One **repo-relative** change record.
///
/// The reader is git-only and workspace-agnostic: it returns repo-relative
/// paths, and the engine strips each workspace's prefix. `old_path` carries the
/// pre-rename/-delete path so the engine can reproduce the delete/rename
/// double-insert affected-detection behaviour.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
pub struct ChangeRecord {
    /// Repo-relative path of the change (the new path on a rename).
    pub path: PathBuf,
    /// What kind of change this is.
    pub status: ChangeStatus,
    /// The previous path on a rename/delete; `None` otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_path: Option<PathBuf>,
}

impl ChangeRecord {
    /// Construct a record with no `old_path`.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>, status: ChangeStatus) -> Self {
        Self {
            path: path.into(),
            status,
            old_path: None,
        }
    }

    /// Attach the previous path (rename/delete).
    #[must_use]
    pub fn with_old_path(mut self, old_path: impl Into<PathBuf>) -> Self {
        self.old_path = Some(old_path.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{ChangeRecord, ChangeStatus};

    #[test]
    fn new_has_no_old_path() {
        let record = ChangeRecord::new("src/lib.rs", ChangeStatus::Modified);
        assert_eq!(record.path, PathBuf::from("src/lib.rs"));
        assert_eq!(record.status, ChangeStatus::Modified);
        assert!(record.old_path.is_none());
    }

    #[test]
    fn with_old_path_attaches_previous_path() {
        let record = ChangeRecord::new("new.rs", ChangeStatus::Renamed).with_old_path("old.rs");
        assert_eq!(record.old_path, Some(PathBuf::from("old.rs")));
    }

    #[test]
    fn round_trips_through_toml() {
        let record = ChangeRecord::new("new.rs", ChangeStatus::Renamed).with_old_path("old.rs");
        let serialized = toml::to_string(&record).expect("serialize");
        let back: ChangeRecord = toml::from_str(&serialized).expect("deserialize");
        assert_eq!(record, back);
    }
}
