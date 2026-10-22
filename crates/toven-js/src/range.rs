//! npm version-range satisfaction on top of [`rskit_version::semver`].
//!
//! npm ranges differ from Cargo requirements in a few spellings: a bare
//! version means exactly that version (Cargo reads it as `^`), comparators are
//! space-separated (Cargo uses `,`), `a - b` is an inclusive hyphen range, and
//! `||` joins alternatives. Each alternative is rewritten into Cargo syntax and
//! matched by the shared semver owner; caret, tilde, wildcard, and prerelease
//! rules already agree between the two.

use rskit_version::semver::{parse_requirement, parse_version};

/// Whether the package `version` satisfies the npm `range`. An unparseable
/// version or range never matches.
pub(crate) fn satisfies(version: &str, range: &str) -> bool {
    let Some(version) = parse_version(strip_v(version.trim())) else {
        return false;
    };
    range.split("||").any(|alternative| {
        to_cargo(alternative.trim())
            .and_then(|requirement| parse_requirement(&requirement))
            .is_some_and(|requirement| requirement.matches(&version))
    })
}

/// Whether `range` is a valid npm range, as opposed to a dist-tag such as
/// `latest`.
pub(crate) fn is_range(range: &str) -> bool {
    range.split("||").all(|alternative| {
        to_cargo(alternative.trim())
            .and_then(|requirement| parse_requirement(&requirement))
            .is_some()
    })
}

/// Rewrite one npm range alternative into a Cargo requirement.
fn to_cargo(alternative: &str) -> Option<String> {
    if alternative.is_empty() {
        return Some("*".to_string());
    }
    if let Some((low, high)) = alternative.split_once(" - ") {
        return Some(format!(
            ">={}, <={}",
            strip_v(low.trim()),
            strip_v(high.trim())
        ));
    }
    let mut comparators = Vec::new();
    let mut pending = String::new();
    for token in alternative.split_whitespace() {
        if token
            .chars()
            .all(|ch| matches!(ch, '<' | '>' | '=' | '^' | '~'))
        {
            // An operator written apart from its version (`>= 1.2.0`).
            pending.push_str(token);
            continue;
        }
        comparators.push(comparator(&format!("{pending}{token}"))?);
        pending.clear();
    }
    if !pending.is_empty() {
        return None;
    }
    Some(comparators.join(", "))
}

/// Rewrite one comparator: wildcards become `*`, and a bare version gets an
/// explicit `=` so it means that version only.
fn comparator(token: &str) -> Option<String> {
    let split = token
        .find(|ch: char| !matches!(ch, '<' | '>' | '=' | '^' | '~'))
        .unwrap_or(token.len());
    let (operator, version) = token.split_at(split);
    let version = strip_v(version)
        .split('.')
        .map(|part| if matches!(part, "x" | "X") { "*" } else { part })
        .collect::<Vec<_>>()
        .join(".");
    if version.is_empty() {
        return None;
    }
    if version == "*" {
        return Some("*".to_string());
    }
    if operator.is_empty() && !version.contains('*') {
        return Some(format!("={version}"));
    }
    Some(format!("{operator}{version}"))
}

/// Drop a leading `v` (`v1.2.3`).
fn strip_v(version: &str) -> &str {
    version.strip_prefix(['v', 'V']).unwrap_or(version)
}

#[cfg(test)]
mod tests {
    use super::{is_range, satisfies};

    #[test]
    fn ranges_are_told_apart_from_dist_tags() {
        for range in [
            "^1.2.0",
            "1.x",
            "*",
            "",
            ">=1 <3",
            "1.0.0 - 2.0.0",
            "~1 || ^2",
        ] {
            assert!(is_range(range), "{range}");
        }
        for tag in ["latest", "next", "beta-1"] {
            assert!(!is_range(tag), "{tag}");
        }
    }

    #[test]
    fn caret_tilde_and_wildcards_follow_npm() {
        assert!(satisfies("1.4.0", "^1.2.0"));
        assert!(!satisfies("2.0.0", "^1.2.0"));
        assert!(satisfies("1.2.9", "~1.2.0"));
        assert!(!satisfies("1.3.0", "~1.2.0"));
        assert!(satisfies("3.1.4", "*"));
        assert!(satisfies("3.1.4", ""));
        assert!(satisfies("1.9.0", "1.x"));
        assert!(!satisfies("2.0.0", "1.x"));
        assert!(satisfies("0.2.5", "^0.2.1"));
        assert!(!satisfies("0.3.0", "^0.2.1"));
    }

    #[test]
    fn bare_versions_are_exact() {
        assert!(satisfies("1.2.3", "1.2.3"));
        assert!(!satisfies("1.2.4", "1.2.3"));
        assert!(satisfies("1.2.4", "1.2"));
        assert!(!satisfies("1.3.0", "1.2"));
        assert!(satisfies("1.2.3", "v1.2.3"));
    }

    #[test]
    fn comparator_sets_hyphens_and_alternatives() {
        assert!(satisfies("1.5.0", ">=1.2.0 <2.0.0"));
        assert!(satisfies("1.5.0", ">= 1.2.0 < 2.0.0"));
        assert!(!satisfies("2.0.0", ">=1.2.0 <2.0.0"));
        assert!(satisfies("2.3.4", "1.0.0 - 2.3.4"));
        assert!(!satisfies("2.3.5", "1.0.0 - 2.3.4"));
        assert!(satisfies("3.0.0", "^1.0.0 || ^3.0.0"));
    }

    #[test]
    fn non_semver_specs_never_match() {
        assert!(!satisfies("1.0.0", "latest"));
        assert!(!satisfies("1.0.0", "github:acme/lib"));
        assert!(!satisfies("not-a-version", "*"));
        assert!(!satisfies("1.0.0", ">="));
    }

    #[test]
    fn prereleases_need_a_matching_tuple() {
        assert!(!satisfies("2.0.0-beta.1", "^1.0.0"));
        assert!(satisfies("2.0.0-beta.2", "^2.0.0-beta.1"));
    }
}
