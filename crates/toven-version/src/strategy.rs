//! Bump-policy name resolution.
//!
//! The bump surface is one matrix rather than a family of named strategies. The
//! `[…release].strategy` config field resolves to a single [`BumpPolicy`]; a
//! prerelease is driven only by `--pre <channel>` / the `prerelease` config,
//! never by a policy name. The semver-increment *math* itself lives in the pure
//! [`toven_semver`] toolkit; this module owns only the policy naming.

use rskit_errors::{AppError, AppResult};

use crate::BumpPolicy;

/// Resolve a configured bump-policy name.
///
/// # Errors
/// Rejects any unknown policy name (including the removed `caret-prerelease`).
pub fn resolve_bump_policy(raw: Option<&str>) -> AppResult<BumpPolicy> {
    match raw.unwrap_or(BumpPolicy::SemverCascade.as_str()) {
        "semver-cascade" => Ok(BumpPolicy::SemverCascade),
        "manifest" => Ok(BumpPolicy::Manifest),
        unknown => Err(AppError::invalid_input(
            "release.strategy",
            format!("unknown bump policy '{unknown}'"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_bump_policy as resolve;
    use crate::BumpPolicy;

    #[test]
    fn resolves_the_single_named_policy_and_defaults() {
        assert_eq!(
            resolve(Some("semver-cascade")).unwrap(),
            BumpPolicy::SemverCascade
        );
        assert_eq!(resolve(Some("manifest")).unwrap(), BumpPolicy::Manifest);
        assert_eq!(resolve(None).unwrap(), BumpPolicy::SemverCascade);
    }

    #[test]
    fn rejects_the_removed_and_unknown_policy_names() {
        assert!(resolve(Some("caret-prerelease")).is_err());
        assert!(resolve(Some("other")).is_err());
    }
}
