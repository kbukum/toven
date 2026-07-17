//! Release changelog planning.

use toven_model::Module;
use toven_ports::ChangeRecord;

use super::ChangelogEntry;

/// Build a deterministic changelog entry from changed records.
///
/// The entry's `breaking` flag stays `false` here: [`ChangeRecord`] carries only a path and status, so a conventional-commit breaking signal has no source at this seam. Breaking is instead driven by the explicit `--minor`/`--major` argv or a per-module config `level`; this is the hook where a commit-message-aware classifier would set it.
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
    ChangelogEntry::new(module.key(), summary, lines)
}
