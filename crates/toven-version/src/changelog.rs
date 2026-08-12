//! Release changelog planning: the [`ChangelogEntry`] value and the pure,
//! forge-agnostic generation, merge, and roll helpers built on Conventional
//! Commit classification.

use std::collections::BTreeMap;

use rskit_version::semver::Version;
use toven_model::{Module, ModuleKey};
use toven_ports::CommitSummary;

use crate::conventional::{ChangeGroup, classify};

/// Human- and machine-consumable changelog summary for a module release.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ChangelogEntry {
    /// Module the entry describes.
    pub module: ModuleKey,
    /// Short summary derived from changed paths/commits.
    pub summary: String,
    /// Detailed lines for later report rendering.
    pub lines: Vec<String>,
    /// Whether the change classification marks this release as breaking.
    pub breaking: bool,
}

impl ChangelogEntry {
    /// Construct a non-breaking changelog entry.
    #[must_use]
    pub fn new(module: ModuleKey, summary: impl Into<String>, lines: Vec<String>) -> Self {
        Self {
            module,
            summary: summary.into(),
            lines,
            breaking: false,
        }
    }

    /// Mark this entry as a breaking change.
    #[must_use]
    pub const fn with_breaking(mut self, breaking: bool) -> Self {
        self.breaking = breaking;
        self
    }
}

/// Build a deterministic, grouped changelog entry from a module's commit range.
///
/// The commits are the module's `baseline..HEAD` history (scoped to its own
/// directory); each is classified as a Conventional Commit and rendered as a
/// Keep a Changelog-style, attributed bullet under its group heading. The
/// result is forge-agnostic — derived from git data alone — so it drives a
/// GitHub or GitLab hosted-release body identically and is fully previewable in
/// a dry run.
///
/// `initial` marks a module that has never been released: with no prior tag to
/// diff against it still carries its whole path history, but an empty range is
/// summarized as an initial release rather than an (equally empty) dependency
/// cascade.
///
/// The entry's `breaking` flag stays driven by explicit `--minor`/`--major`
/// argv or per-module config `level`, not by this seam: commit-derived breaking
/// markers surface in the rendered notes (a `Breaking changes` section) without
/// silently re-deciding the version bump.
#[must_use]
pub fn entry(module: &Module, commits: &[CommitSummary], initial: bool) -> ChangelogEntry {
    let mut grouped: BTreeMap<ChangeGroup, Vec<String>> = BTreeMap::new();
    for commit in commits {
        let classified = classify(commit);
        grouped
            .entry(classified.group)
            .or_default()
            .push(render_bullet(&classified));
    }

    let mut lines = Vec::new();
    for group in ChangeGroup::ordered() {
        let Some(bullets) = grouped.get(&group) else {
            continue;
        };
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(format!("### {}", group.heading()));
        lines.extend(bullets.iter().cloned());
    }

    let summary = if commits.is_empty() {
        if initial {
            "initial release".to_string()
        } else {
            "dependency cascade".to_string()
        }
    } else {
        let plural = if commits.len() == 1 { "" } else { "s" };
        format!("{} commit{plural}", commits.len())
    };
    ChangelogEntry::new(module.key(), summary, lines)
}

/// Render one classified commit as a Keep a Changelog bullet with its scope,
/// author attribution, and short id: `- **scope**: description — by @handle
/// (id)` (the `**scope**: ` prefix is omitted when the commit has no scope).
fn render_bullet(commit: &crate::conventional::ClassifiedCommit) -> String {
    use std::fmt::Write as _;
    let mut bullet = String::from("- ");
    if let Some(scope) = &commit.scope {
        let _ = write!(bullet, "**{scope}**: ");
    }
    bullet.push_str(&commit.description);
    let _ = write!(bullet, " — by {} ({})", commit.author, commit.id);
    bullet
}

/// Merge two rendered changelog note bodies into one, unioning their sections by
/// heading and de-duplicating bullets.
///
/// A single-version workspace maps every module onto one hosted Release, so its
/// per-module note bodies (each already grouped and in canonical order) are
/// folded together here rather than blindly concatenated — which would repeat a
/// `### Added` heading once per contributing module. Sections are keyed by
/// heading, bullets keep first-seen order with exact duplicates dropped, and the
/// unioned sections are re-sorted into canonical group order
/// (Breaking/Added/Fixed/Changed/Other) so a heading only present in one input
/// still lands in its canonical slot rather than after the other input's
/// sections. Any unrecognized/headingless fallback section sorts first.
#[must_use]
pub fn merge_notes(existing: &str, incoming: &str) -> String {
    if existing.trim().is_empty() {
        return incoming.to_string();
    }
    if incoming.trim().is_empty() {
        return existing.to_string();
    }

    let mut sections = parse_sections(existing);
    for (heading, bullets) in parse_sections(incoming) {
        if let Some((_, present)) = sections.iter_mut().find(|(name, _)| *name == heading) {
            for bullet in bullets {
                if !present.contains(&bullet) {
                    present.push(bullet);
                }
            }
        } else {
            sections.push((heading, bullets));
        }
    }
    // `ChangeGroup` derives `Ord` in canonical render order, and `sort_by_key`
    // is stable, so known headings land in canonical order while any headingless
    // fallback (`None`) sorts first and ties keep first-seen order.
    sections.sort_by_key(|(heading, _)| ChangeGroup::from_heading(heading));
    render_sections(&sections)
}

/// Parse a rendered note body into ordered `(heading, bullets)` sections.
///
/// Only the two line shapes this module emits are recognized — `### Heading`
/// starts a section and any other non-blank line is a bullet under the current
/// heading — so parsing round-trips our own output. Content before the first
/// heading (a bare fallback line) becomes a leading headingless section.
fn parse_sections(notes: &str) -> Vec<(String, Vec<String>)> {
    let mut sections: Vec<(String, Vec<String>)> = Vec::new();
    for line in notes.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(heading) = trimmed.strip_prefix("### ") {
            sections.push((heading.to_string(), Vec::new()));
        } else if let Some((_, bullets)) = sections.last_mut() {
            bullets.push(trimmed.to_string());
        } else {
            sections.push((String::new(), vec![trimmed.to_string()]));
        }
    }
    sections
}

/// Render ordered `(heading, bullets)` sections back into a note body, matching
/// [`entry`]'s layout: a blank line between sections and `### ` before each
/// non-empty heading.
fn render_sections(sections: &[(String, Vec<String>)]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for (heading, bullets) in sections {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        if !heading.is_empty() {
            lines.push(format!("### {heading}"));
        }
        lines.extend(bullets.iter().cloned());
    }
    lines.join("\n")
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
pub fn unreleased_documented(text: &str) -> bool {
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

/// Whether a `## ` line is the `[Unreleased]` heading.
fn is_unreleased_heading(line: &str) -> bool {
    line.trim().strip_prefix("## ").is_some_and(|heading| {
        let heading = heading.trim();
        heading.contains('[') && heading.to_ascii_lowercase().contains("unreleased")
    })
}

/// Roll a documented `## [Unreleased]` section into a versioned
/// `## [version] - date` section, returning the rewritten document.
///
/// This is the `bump` phase's changelog **rewrite** (as distinct from the
/// plan-time `unreleased_documented` **verify**): the accumulated `[Unreleased]`
/// body is relocated verbatim under a new `## [version] - date` heading while an
/// empty `## [Unreleased]` heading is left in place to collect the next cycle's
/// entries. It never fabricates prose — only the entries an author already
/// recorded move. Returns `None` when there is nothing to roll (no
/// `[Unreleased]` section, or one with no documented entry), so a caller can
/// leave the file untouched rather than write an empty versioned section.
#[must_use]
pub fn roll_unreleased(text: &str, version: &Version, date: &str) -> Option<String> {
    if !unreleased_documented(text) {
        return None;
    }
    let lines: Vec<&str> = text.lines().collect();
    let heading_index = lines.iter().position(|line| is_unreleased_heading(line))?;
    // The section body runs from just after the `[Unreleased]` heading to the
    // next level-2 heading (or the end of the document).
    let body_end = lines[heading_index + 1..]
        .iter()
        .position(|line| line.trim_start().starts_with("## "))
        .map_or(lines.len(), |offset| heading_index + 1 + offset);

    let body: Vec<&str> = trim_blank_edges(&lines[heading_index + 1..body_end]);
    let suffix = &lines[body_end..];

    let mut rolled: Vec<String> = lines[..=heading_index]
        .iter()
        .map(|line| (*line).to_string())
        .collect();
    rolled.push(String::new());
    rolled.push(format!("## [{version}] - {date}"));
    rolled.push(String::new());
    rolled.extend(body.iter().map(|line| (*line).to_string()));
    if !suffix.is_empty() {
        rolled.push(String::new());
        rolled.extend(suffix.iter().map(|line| (*line).to_string()));
    }
    let mut out = rolled.join("\n");
    out.push('\n');
    Some(out)
}

/// Drop leading and trailing blank lines from a slice of lines.
fn trim_blank_edges<'a>(lines: &[&'a str]) -> Vec<&'a str> {
    let start = lines
        .iter()
        .position(|line| !line.trim().is_empty())
        .unwrap_or(lines.len());
    let end = lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .map_or(start, |index| index + 1);
    lines[start..end].to_vec()
}

#[cfg(test)]
mod tests {
    use super::{merge_notes, roll_unreleased, unreleased_documented};
    use rskit_version::semver::Version;

    #[test]
    fn merging_same_heading_unions_bullets_under_one_section() {
        let existing = "### Added\n- one";
        let incoming = "### Added\n- two";
        assert_eq!(merge_notes(existing, incoming), "### Added\n- one\n- two");
    }

    #[test]
    fn merging_drops_duplicate_bullets() {
        let existing = "### Added\n- shared\n- one";
        let incoming = "### Added\n- shared\n- two";
        assert_eq!(
            merge_notes(existing, incoming),
            "### Added\n- shared\n- one\n- two"
        );
    }

    #[test]
    fn merging_distinct_headings_keeps_both_in_order() {
        let existing = "### Added\n- a";
        let incoming = "### Fixed\n- b";
        assert_eq!(
            merge_notes(existing, incoming),
            "### Added\n- a\n\n### Fixed\n- b"
        );
    }

    #[test]
    fn merging_re_sorts_unioned_sections_into_canonical_order() {
        // The incoming `Fixed` heading must land between `Added` and `Other`,
        // not append after the existing sections in first-seen order.
        let existing = "### Added\n- a\n\n### Other\n- c";
        let incoming = "### Fixed\n- b";
        assert_eq!(
            merge_notes(existing, incoming),
            "### Added\n- a\n\n### Fixed\n- b\n\n### Other\n- c"
        );
    }

    #[test]
    fn merging_with_an_empty_side_returns_the_other() {
        assert_eq!(merge_notes("", "### Added\n- a"), "### Added\n- a");
        assert_eq!(merge_notes("### Added\n- a", ""), "### Added\n- a");
    }

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

    #[test]
    fn rolling_moves_the_unreleased_body_under_a_versioned_heading() {
        let text = "# Changelog\n\n## [Unreleased]\n\n### Added\n\n- A feature\n\n## \
                    [1.0.0] - 2026-01-01\n\n- Older change\n";
        let rolled = roll_unreleased(text, &Version::new(1, 1, 0), "2026-08-04").expect("rolls");
        assert_eq!(
            rolled,
            "# Changelog\n\n## [Unreleased]\n\n## [1.1.0] - 2026-08-04\n\n### Added\n\n- A \
             feature\n\n## [1.0.0] - 2026-01-01\n\n- Older change\n"
        );
        // The rolled document leaves an empty `[Unreleased]` for the next cycle.
        assert!(!unreleased_documented(&rolled));
    }

    #[test]
    fn rolling_an_undocumented_unreleased_section_is_a_noop() {
        let text = "## [Unreleased]\n\n### Added\n\n## [1.0.0]\n\n- Shipped\n";
        assert!(roll_unreleased(text, &Version::new(1, 1, 0), "2026-08-04").is_none());
    }

    #[test]
    fn rolling_a_trailing_unreleased_section_needs_no_following_section() {
        let text = "## [Unreleased]\n\n- Only change\n";
        let rolled = roll_unreleased(text, &Version::new(0, 2, 0), "2026-08-04").expect("rolls");
        assert_eq!(
            rolled,
            "## [Unreleased]\n\n## [0.2.0] - 2026-08-04\n\n- Only change\n"
        );
    }
}
