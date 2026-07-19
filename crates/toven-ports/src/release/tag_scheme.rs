//! Per-target release tag grammar.

use rskit_version::semver::Version;

/// A release tag grammar that surrounds a semantic version with fixed text.
///
/// Ecosystem targets own how this scheme is constructed for each module. The
/// engine only formats and parses through the returned value, so Rust can use
/// `rust/core@1.2.3` while Go can use `cache/redis/v1.2.3`.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct TagScheme {
    prefix: String,
    suffix: String,
}

impl TagScheme {
    /// Construct a tag scheme from the fixed text before and after the version.
    #[must_use]
    pub fn new(prefix: impl Into<String>, suffix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            suffix: suffix.into(),
        }
    }

    /// Format `version` as a release tag.
    #[must_use]
    pub fn format(&self, version: &Version) -> String {
        format!("{}{version}{}", self.prefix, self.suffix)
    }

    /// Parse a release tag that matches this scheme.
    #[must_use]
    pub fn parse(&self, tag: &str) -> Option<Version> {
        let without_prefix = tag.strip_prefix(&self.prefix)?;
        let raw_version = without_prefix.strip_suffix(&self.suffix)?;
        Version::parse(raw_version).ok()
    }
}

#[cfg(test)]
mod tests {
    use rskit_version::semver::Version;

    use super::TagScheme;

    #[test]
    fn formats_and_parses_round_trip() {
        let scheme = TagScheme::new("rust/core@", "");
        let version = Version::new(1, 2, 3);

        let tag = scheme.format(&version);

        assert_eq!(tag, "rust/core@1.2.3");
        assert_eq!(scheme.parse(&tag), Some(version));
    }

    #[test]
    fn non_matching_prefix_returns_none() {
        let scheme = TagScheme::new("rust/core@", "");

        assert_eq!(scheme.parse("go/core@1.2.3"), None);
    }

    #[test]
    fn empty_suffix_is_supported() {
        let scheme = TagScheme::new("cache/redis/v", "");

        assert_eq!(
            scheme.parse("cache/redis/v1.2.3"),
            Some(Version::new(1, 2, 3))
        );
    }
}
