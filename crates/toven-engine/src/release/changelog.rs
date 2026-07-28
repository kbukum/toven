//! Release changelog planning.

use toven_model::Module;
use toven_ports::ChangeRecord;

use super::ChangelogEntry;

/// Build a deterministic changelog entry from changed records.
///
/// `initial` marks a module that has never been released: it has no prior
/// release tag to diff against, so it carries no changed records and is
/// summarized as an initial release rather than as an (equally record-free)
/// dependency cascade.
///
/// The entry's `breaking` flag stays `false` here: [`ChangeRecord`] carries
/// only a path and status, so a conventional-commit breaking signal has no
/// source at this seam. Breaking is instead driven by the explicit
/// `--minor`/`--major` argv or a per-module config `level`; this is the hook
/// where a commit-message-aware classifier would set it.
#[must_use]
pub(super) fn entry(module: &Module, changes: &[ChangeRecord], initial: bool) -> ChangelogEntry {
    let mut lines = changes
        .iter()
        .map(|change| change.path.display().to_string())
        .collect::<Vec<_>>();
    lines.sort();
    lines.dedup();

    let summary = if !lines.is_empty() {
        format!("{} changed path(s)", lines.len())
    } else if initial {
        "initial release".to_string()
    } else {
        "dependency cascade".to_string()
    };
    ChangelogEntry::new(module.key(), summary, lines)
}

/// Whether a [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) document
/// carries a documented `## [Unreleased]` section.
///
/// A section counts as documented when at least one non-blank, non-heading line
/// (a bullet or prose entry) appears under the `## [Unreleased]` level-2 heading
/// before the next level-2 heading. An absent `[Unreleased]` section, or one
/// that holds only empty subsection headings such as `### Added`, is treated as
/// undocumented so a required-changelog release fails closed.
#[must_use]
pub(super) fn unreleased_documented(text: &str) -> bool {
    let mut in_unreleased = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(heading) = trimmed.strip_prefix("## ") {
            let heading = heading.trim();
            in_unreleased =
                heading.contains('[') && heading.to_ascii_lowercase().contains("unreleased");
            continue;
        }
        if in_unreleased && !trimmed.is_empty() && !trimmed.starts_with('#') {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::unreleased_documented;

    #[test]
    fn a_populated_unreleased_section_is_documented() {
        let text = "# Changelog\n\n## [Unreleased]\n\n### Added\n\n- A new feature\n\n## \
                    [1.0.0]\n\n- Older change\n";
        assert!(unreleased_documented(text));
    }

    #[test]
    fn prose_under_unreleased_counts_as_documented() {
        let text = "## [Unreleased]\nReworked the planner end to end.\n";
        assert!(unreleased_documented(text));
    }

    #[test]
    fn an_empty_unreleased_section_is_not_documented() {
        // Only subsection headings, no entries: nothing was actually recorded.
        let text = "## [Unreleased]\n\n### Added\n\n### Fixed\n\n## [1.0.0]\n\n- Shipped\n";
        assert!(!unreleased_documented(text));
    }

    #[test]
    fn a_missing_unreleased_section_is_not_documented() {
        let text = "# Changelog\n\n## [1.0.0]\n\n- Shipped\n";
        assert!(!unreleased_documented(text));
    }

    #[test]
    fn entries_only_under_a_released_section_do_not_count() {
        // A bullet after Unreleased closes belongs to the released section.
        let text = "## [Unreleased]\n\n## [1.0.0]\n\n- Shipped\n";
        assert!(!unreleased_documented(text));
    }
}
