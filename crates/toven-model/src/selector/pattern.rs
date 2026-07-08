//! The module-name segment of a selector: an exact name or a shell-style glob.

use std::fmt;

use rskit_util::glob::{Glob, has_wildcard};

/// A pattern over a single module-name segment.
///
/// Carries the parsed shape of the rightmost segment of a selector before it is
/// resolved against a graph: either an exact name (matches one identity) or a
/// shell-style glob (`*`/`?`, an explicit set that may match many). It is pure
/// vocabulary — it holds no resolution logic and never touches a graph; matching
/// a candidate name is a plain string test.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum NamePattern {
    /// An exact module name; matches exactly the equal name.
    Exact(String),
    /// A shell-style glob (`*`/`?`); matches every name it covers.
    Glob(Glob),
}

impl NamePattern {
    /// Parse a name segment into an exact name or a glob.
    ///
    /// A segment containing `*` or `?` becomes a [`Glob`](NamePattern::Glob); any
    /// other segment is an [`Exact`](NamePattern::Exact) name. The segment is not
    /// otherwise validated here — resolution against the graph decides whether it
    /// matches, so the lenient input boundary never rejects a syntactically odd
    /// but resolvable token.
    #[must_use]
    pub fn parse(segment: &str) -> Self {
        if has_wildcard(segment) {
            Self::Glob(Glob::new(segment))
        } else {
            Self::Exact(segment.to_owned())
        }
    }

    /// Whether this pattern is an exact name (not a glob).
    #[must_use]
    pub const fn is_exact(&self) -> bool {
        matches!(self, Self::Exact(_))
    }

    /// Whether `name` matches this pattern.
    #[must_use]
    pub fn matches(&self, name: &str) -> bool {
        match self {
            Self::Exact(exact) => exact == name,
            Self::Glob(glob) => glob.matches(name),
        }
    }
}

impl fmt::Display for NamePattern {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exact(exact) => formatter.write_str(exact),
            Self::Glob(glob) => formatter.write_str(glob.pattern()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::NamePattern;

    #[test]
    fn plain_name_parses_as_exact() {
        let pattern = NamePattern::parse("core");
        assert!(pattern.is_exact());
        assert!(pattern.matches("core"));
        assert!(!pattern.matches("cores"));
    }

    #[test]
    fn wildcard_name_parses_as_glob() {
        let pattern = NamePattern::parse("rskit-*");
        assert!(!pattern.is_exact());
        assert!(pattern.matches("rskit-errors"));
        assert!(!pattern.matches("toven-core"));
    }

    #[test]
    fn question_mark_is_a_glob() {
        let pattern = NamePattern::parse("co?e");
        assert!(!pattern.is_exact());
        assert!(pattern.matches("core"));
        assert!(!pattern.matches("code2"));
    }

    #[test]
    fn renders_its_source() {
        assert_eq!(NamePattern::parse("core").to_string(), "core");
        assert_eq!(NamePattern::parse("rust-*").to_string(), "rust-*");
    }
}
