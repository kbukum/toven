//! Workspace member patterns: per-segment globs with `**` and `!` exclusions.
//!
//! npm, Yarn, Bun, and pnpm all select members with directory globs relative
//! to the root. Each `/`-separated segment matches with
//! [`rskit_util::glob::glob_match`] (`*` and `?` never cross a `/`), a `**`
//! segment matches zero or more whole segments, and a `!`-prefixed pattern
//! excludes what it matches.

use std::path::{Component, Path};

use rskit_errors::{AppError, AppResult};
use rskit_util::glob::glob_match;

/// The double-star segment matching zero or more whole segments.
const GLOBSTAR: &str = "**";

/// The parsed member patterns of one root.
#[derive(Debug, Clone, Default)]
pub(crate) struct MemberPatterns {
    /// Patterns selecting members, split into segments.
    include: Vec<Vec<String>>,
    /// `!` patterns removing members, split into segments.
    exclude: Vec<Vec<String>>,
}

impl MemberPatterns {
    /// Parse `patterns` declared by `source` (a repo-relative file label).
    ///
    /// # Errors
    /// A pattern that is absolute or climbs out of the root with `..`.
    pub(crate) fn parse(source: &str, patterns: &[String]) -> AppResult<Self> {
        let mut parsed = Self::default();
        for pattern in patterns {
            let (negated, body) = pattern
                .strip_prefix('!')
                .map_or((false, pattern.as_str()), |body| (true, body));
            let segments = segments(source, pattern, body)?;
            if segments.is_empty() {
                // `.` names the root itself, which is never its own member.
                continue;
            }
            if negated {
                parsed.exclude.push(segments);
            } else {
                parsed.include.push(segments);
            }
        }
        Ok(parsed)
    }

    /// Whether the patterns select at least one member shape.
    pub(crate) const fn is_empty(&self) -> bool {
        self.include.is_empty()
    }

    /// Whether the directory at `relative` (relative to the root) is a member.
    pub(crate) fn matches(&self, relative: &Path) -> bool {
        let path: Vec<String> = relative
            .components()
            .filter_map(|component| match component {
                Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect();
        if path.is_empty() {
            return false;
        }
        self.include.iter().any(|pattern| matches(pattern, &path))
            && !self.exclude.iter().any(|pattern| matches(pattern, &path))
    }
}

/// Split one pattern body into normalized segments.
fn segments(source: &str, pattern: &str, body: &str) -> AppResult<Vec<String>> {
    if body.starts_with('/') || body.starts_with('\\') {
        return Err(escape(source, pattern));
    }
    let mut segments = Vec::new();
    for segment in body.split(['/', '\\']) {
        match segment {
            "" | "." => {}
            ".." => return Err(escape(source, pattern)),
            _ => segments.push(segment.to_string()),
        }
    }
    Ok(segments)
}

/// A pattern escaping its root.
fn escape(source: &str, pattern: &str) -> AppError {
    AppError::invalid_input(
        "workspaces",
        format!(
            "'{source}' workspace pattern '{pattern}' escapes its root; members must live under \
             the root directory"
        ),
    )
}

/// Whether `pattern` segments match `path` segments.
fn matches(pattern: &[String], path: &[String]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((head, rest)) if head == GLOBSTAR => {
            (0..=path.len()).any(|skip| matches(rest, &path[skip..]))
        }
        Some((head, rest)) => path
            .split_first()
            .is_some_and(|(segment, tail)| glob_match(head, segment) && matches(rest, tail)),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::MemberPatterns;

    fn patterns(list: &[&str]) -> MemberPatterns {
        let owned: Vec<String> = list.iter().map(ToString::to_string).collect();
        MemberPatterns::parse("package.json", &owned).expect("patterns")
    }

    #[test]
    fn star_matches_one_segment_only() {
        let set = patterns(&["packages/*"]);
        assert!(set.matches(Path::new("packages/a")));
        assert!(!set.matches(Path::new("packages/a/b")));
        assert!(!set.matches(Path::new("apps/a")));
        assert!(!set.matches(Path::new("")));
    }

    #[test]
    fn globstar_matches_any_depth() {
        let set = patterns(&["./libs/**/", "tools/*-cli"]);
        assert!(set.matches(Path::new("libs/a")));
        assert!(set.matches(Path::new("libs/a/b/c")));
        assert!(set.matches(Path::new("tools/gen-cli")));
        assert!(!set.matches(Path::new("tools/gen")));
    }

    #[test]
    fn negation_excludes_members() {
        let set = patterns(&["packages/*", "!packages/skip", "."]);
        assert!(set.matches(Path::new("packages/keep")));
        assert!(!set.matches(Path::new("packages/skip")));
        assert!(!set.is_empty());
        assert!(patterns(&["."]).is_empty());
    }

    #[test]
    fn escaping_patterns_are_rejected() {
        for pattern in ["../shared/*", "/abs/*", "packages/../../x"] {
            let error = MemberPatterns::parse("web/package.json", &[pattern.to_string()])
                .expect_err(pattern);
            assert!(error.to_string().contains("escapes its root"), "{error}");
        }
    }
}
