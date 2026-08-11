//! Tag listing + glob filtering for the adapter's `list_tags`.
//!
//! rskit-git's [`RefManager::list_tags`](rskit_git::RefManager) returns every
//! tag; release change-detection asks for `"<module>@*"`, so this filters with
//! a minimal `*`/`?` glob (no glob crate in the dependency set) and maps
//! survivors onto the ports' [`TagRef`]. The max-semver tag selection lives in
//! the pure [`toven_semver::latest_matching`] toolkit function.

use rskit_errors::AppResult;
use rskit_git::{RefManager, Repo};
use toven_ports::TagRef;

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
    use super::glob_match;

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
}
