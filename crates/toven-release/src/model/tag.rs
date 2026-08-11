//! Release tag formatting and latest-tag selection.

use rskit_version::semver::Version;
use toven_ports::{TagRef, TagScheme};
use toven_semver::latest_matching;

/// Format a release tag through the target-owned scheme.
#[must_use]
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn format(scheme: &TagScheme, version: &Version) -> String {
    scheme.format(version)
}

/// Select the newest semver tag matched by `scheme`.
///
/// A thin delegating wrapper over the change foundation's shared
/// [`latest_matching`] so the max-semver selection has a single home.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn latest(scheme: &TagScheme, tags: &[TagRef]) -> Option<(Version, TagRef)> {
    latest_matching(scheme, tags)
}

#[cfg(test)]
mod tests {
    use rskit_version::semver::Version;
    use toven_ports::{Oid, TagRef, TagScheme};

    use super::latest;

    #[test]
    fn tag_grammar_uses_target_owned_scheme() {
        let scheme = TagScheme::new("rust/core@", "");
        let version = Version::new(1, 2, 3);

        assert_eq!(super::format(&scheme, &version), "rust/core@1.2.3");
        assert_eq!(scheme.parse("rust/core@1.2.3"), Some(version));
        assert_eq!(scheme.parse("core@1.2.3"), None);
    }

    #[test]
    fn latest_ignores_non_matching_tags() {
        let scheme = TagScheme::new("rust/core@", "");
        let tags = vec![
            TagRef::new("rust/core@0.1.0", Oid::new("a")),
            TagRef::new("go/core@9.9.9", Oid::new("b")),
            TagRef::new("rust/core@0.2.0", Oid::new("c")),
        ];

        let (version, tag) = latest(&scheme, &tags).expect("latest tag");

        assert_eq!(version, Version::new(0, 2, 0));
        assert_eq!(tag.name, "rust/core@0.2.0");
    }
}
