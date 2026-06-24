//! Release changelog planning.

use toven_model::Module;
use toven_ports::ChangeRecord;

use super::ChangelogEntry;

/// Build a deterministic changelog entry from changed records.
#[must_use]
pub(super) fn entry(module: &Module, changes: &[ChangeRecord]) -> ChangelogEntry {
    let mut lines = changes
        .iter()
        .map(|change| change.path.display().to_string())
        .collect::<Vec<_>>();
    lines.sort();
    lines.dedup();

    let summary = if lines.is_empty() {
        "dependency cascade".to_string()
    } else {
        format!("{} changed path(s)", lines.len())
    };
    ChangelogEntry::new(module.id.clone(), summary, lines)
}
