//! Max-semver tag selection.
//!
//! [`latest_matching`] is the single home of the "newest matching tag" pick:
//! parse each candidate's name through a [`TagScheme`](crate::TagScheme), keep
//! the matches, and return the highest version. It is generic over any
//! [`Tagged`] item so callers keep their own tag-reference types (git tag refs,
//! registry entries, …) without this crate depending on them.

use rskit_version::semver::Version;

use crate::TagScheme;

/// A candidate a [`TagScheme`] can select by its tag name.
///
/// Implement this on a tag-reference type to make it selectable by
/// [`latest_matching`] without exposing that type to this crate.
pub trait Tagged {
    /// The candidate's tag name (e.g. `rust/core@1.2.3`).
    fn tag_name(&self) -> &str;
}

/// Select the newest semver tag matched by `scheme`.
///
/// Parses each candidate's [`tag_name`](Tagged::tag_name) through `scheme`,
/// keeps the matches, and returns the highest version paired with the winning
/// candidate. Reusable wherever a max-semver tag pick is needed so no caller
/// reimplements the selection.
#[must_use]
pub fn latest_matching<T: Tagged + Clone>(scheme: &TagScheme, tags: &[T]) -> Option<(Version, T)> {
    tags.iter()
        .filter_map(|tag| {
            scheme
                .parse(tag.tag_name())
                .map(|version| (version, tag.clone()))
        })
        .max_by(|(left, _), (right, _)| left.cmp(right))
}

#[cfg(test)]
mod tests {
    use rskit_version::semver::Version;

    use super::{Tagged, latest_matching};
    use crate::TagScheme;

    #[derive(Clone)]
    struct Tag(&'static str);

    impl Tagged for Tag {
        fn tag_name(&self) -> &str {
            self.0
        }
    }

    #[test]
    fn latest_matching_picks_the_highest_matching_semver() {
        let scheme = TagScheme::new("rust/core@", "");
        let tags = vec![
            Tag("rust/core@0.1.0"),
            Tag("go/core@9.9.9"),
            Tag("rust/core@0.2.0"),
        ];

        let (version, tag) = latest_matching(&scheme, &tags).expect("latest tag");

        assert_eq!(version, Version::new(0, 2, 0));
        assert_eq!(tag.tag_name(), "rust/core@0.2.0");
    }

    #[test]
    fn latest_matching_returns_none_when_no_tag_matches() {
        let scheme = TagScheme::new("rust/core@", "");
        let tags = vec![Tag("go/core@1.0.0")];

        assert!(latest_matching(&scheme, &tags).is_none());
    }
}
