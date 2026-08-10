//! Tag listing + glob filtering for the adapter's `list_tags`.
//!
//! rskit-git's [`RefManager::list_tags`](rskit_git::RefManager) returns every
//! tag; release change-detection asks for `"<module>@*"`, so this filters with
//! a minimal `*`/`?` glob (no glob crate in the dependency set) and maps
//! survivors onto the ports' [`TagRef`].

use rskit_errors::AppResult;
use rskit_git::{RefManager, Repo};
use rskit_version::semver::Version;
use toven_ports::{TagRef, TagScheme};

use super::convert::to_oid;

/// List tags, optionally filtered by a `*`/`?` glob over the tag name.
pub(super) fn list_tags(repo: &Repo, pattern: Option<&str>) -> AppResult<Vec<TagRef>> {
    Ok(repo
        .list_tags()?
        .into_iter()
        .filter(|tag| pattern.is_none_or(|glob| glob_match(glob, &tag.name)))
        .map(|tag| TagRef::new(tag.name, to_oid(&tag.target)))
        .collect())
}

/// Select the newest semver tag matched by `scheme`.
///
/// The single home of the max-semver tag selection: parse each tag through
/// `scheme`, keep the matches, and pick the highest version. Reusable across
/// the change foundation and the release engine so neither reimplements the
/// selection.
#[must_use]
pub fn latest_matching(scheme: &TagScheme, tags: &[TagRef]) -> Option<(Version, TagRef)> {
    tags.iter()
        .filter_map(|tag| {
            scheme
                .parse(&tag.name)
                .map(|version| (version, tag.clone()))
        })
        .max_by(|(left, _), (right, _)| left.cmp(right))
}

/// Match `name` against a minimal shell glob: `*` spans any run, `?` one char,
/// everything else is literal.
fn glob_match(pattern: &str, name: &str) -> bool {
    let pattern = pattern.chars().collect::<Vec<_>>();
    let name = name.chars().collect::<Vec<_>>();
    matches_from(&pattern, &name)
}

/// Backtracking matcher over char slices (inputs are short tag/glob strings).
fn matches_from(pattern: &[char], name: &[char]) -> bool {
    match pattern.split_first() {
        None => name.is_empty(),
        Some((&'*', rest)) => {
            // `*` matches zero-or-more: try every suffix of `name`.
            (0..=name.len()).any(|skip| matches_from(rest, &name[skip..]))
        }
        Some((&'?', rest)) => !name.is_empty() && matches_from(rest, &name[1..]),
        Some((&literal, rest)) => {
            name.first().is_some_and(|&head| head == literal) && matches_from(rest, &name[1..])
        }
    }
}

#[cfg(test)]
mod tests {
    use rskit_version::semver::Version;
    use toven_ports::{Oid, TagRef, TagScheme};

    use super::{glob_match, latest_matching};

    #[test]
    fn prefix_star_matches_module_tags() {
        assert!(glob_match("errors@*", "errors@1.2.0"));
        assert!(glob_match("errors@*", "errors@"));
        assert!(!glob_match("errors@*", "config@1.0.0"));
    }

    #[test]
    fn question_mark_matches_single_char() {
        assert!(glob_match("v?", "v1"));
        assert!(!glob_match("v?", "v12"));
    }

    #[test]
    fn literal_requires_exact_match() {
        assert!(glob_match("v1.0.0", "v1.0.0"));
        assert!(!glob_match("v1.0.0", "v1.0.1"));
    }

    #[test]
    fn star_matches_empty_and_internal() {
        assert!(glob_match("*", ""));
        assert!(glob_match("a*c", "abbbc"));
        assert!(glob_match("a*c", "ac"));
        assert!(!glob_match("a*c", "ab"));
    }

    #[test]
    fn latest_matching_picks_the_highest_matching_semver() {
        let scheme = TagScheme::new("rust/core@", "");
        let tags = vec![
            TagRef::new("rust/core@0.1.0", Oid::new("a")),
            TagRef::new("go/core@9.9.9", Oid::new("b")),
            TagRef::new("rust/core@0.2.0", Oid::new("c")),
        ];

        let (version, tag) = latest_matching(&scheme, &tags).expect("latest tag");

        assert_eq!(version, Version::new(0, 2, 0));
        assert_eq!(tag.name, "rust/core@0.2.0");
    }

    #[test]
    fn latest_matching_returns_none_when_no_tag_matches() {
        let scheme = TagScheme::new("rust/core@", "");
        let tags = vec![TagRef::new("go/core@1.0.0", Oid::new("a"))];

        assert!(latest_matching(&scheme, &tags).is_none());
    }
}
