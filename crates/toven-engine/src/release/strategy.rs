//! Release strategy selection and semver increments.

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_version::semver::Version;

use super::ReleaseStrategyName;

/// Resolve a configured release strategy name.
pub(super) fn resolve(raw: Option<&str>) -> AppResult<ReleaseStrategyName> {
    match raw.unwrap_or(ReleaseStrategyName::SemverCascade.as_str()) {
        "semver-cascade" => Ok(ReleaseStrategyName::SemverCascade),
        "caret-prerelease" => Ok(ReleaseStrategyName::CaretPrerelease),
        unknown => Err(AppError::invalid_input(
            "release.strategy",
            format!("unknown release strategy '{unknown}'"),
        )),
    }
}

/// Compute the next version for a strategy.
///
/// `semver-cascade` finalizes a prerelease to its release (`1.0.0-rc.1` →
/// `1.0.0`) and bumps the patch of a stable version (`1.2.3` → `1.2.4`).
/// `caret-prerelease` keeps a prerelease train together by advancing its numeric
/// tail (`1.0.0-rc.1` → `1.0.0-rc.2`) and bumps the patch of a stable version.
///
/// # Errors
/// Returns an error only if advancing a prerelease train fails to produce a
/// valid semantic version (not expected for registry-sourced versions).
pub(super) fn next_version(strategy: ReleaseStrategyName, current: &Version) -> AppResult<Version> {
    match strategy {
        ReleaseStrategyName::SemverCascade => Ok(semver_cascade(current)),
        ReleaseStrategyName::CaretPrerelease => caret_prerelease(current),
    }
}

/// A stable version's standard patch bump, dropping any build metadata.
const fn patch_bump(current: &Version) -> Version {
    Version::new(
        current.major,
        current.minor,
        current.patch.saturating_add(1),
    )
}

/// `semver-cascade`: a prerelease finalizes to its release; a stable version
/// bumps the patch.
fn semver_cascade(current: &Version) -> Version {
    if current.pre.is_empty() {
        patch_bump(current)
    } else {
        // Promote the prerelease to its final release, dropping pre/build.
        Version::new(current.major, current.minor, current.patch)
    }
}

/// `caret-prerelease`: keep a prerelease train together by advancing its numeric
/// tail; a stable version bumps the patch.
fn caret_prerelease(current: &Version) -> AppResult<Version> {
    if current.pre.is_empty() {
        return Ok(patch_bump(current));
    }
    let next_pre = increment_prerelease(current.pre.as_str());
    let raw = format!(
        "{}.{}.{}-{next_pre}",
        current.major, current.minor, current.patch
    );
    Version::parse(&raw).map_err(|error| {
        AppError::new(
            ErrorCode::InvalidFormat,
            format!("failed to advance prerelease train for '{current}'"),
        )
        .with_cause(error)
    })
}

/// Advance the trailing dot-separated numeric identifier of a prerelease train,
/// or append `.1` when the train has no numeric tail (`rc` → `rc.1`,
/// `rc.1` → `rc.2`, `alpha.3` → `alpha.4`).
fn increment_prerelease(pre: &str) -> String {
    match pre.rsplit_once('.') {
        Some((head, tail)) => tail.parse::<u64>().map_or_else(
            |_| format!("{pre}.1"),
            |number| format!("{head}.{}", number.saturating_add(1)),
        ),
        None => pre.parse::<u64>().map_or_else(
            |_| format!("{pre}.1"),
            |number| number.saturating_add(1).to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use rskit_version::semver::Version;

    use super::{next_version, resolve};
    use crate::release::ReleaseStrategyName;

    fn parse(raw: &str) -> Version {
        Version::parse(raw).unwrap()
    }

    #[test]
    fn resolves_known_strategy_names() {
        assert_eq!(
            resolve(Some("semver-cascade")).unwrap(),
            ReleaseStrategyName::SemverCascade
        );
        assert_eq!(
            resolve(Some("caret-prerelease")).unwrap(),
            ReleaseStrategyName::CaretPrerelease
        );
        assert!(resolve(Some("other")).is_err());
    }

    #[test]
    fn defaults_to_semver_cascade_and_bumps_patch() {
        let strategy = resolve(None).unwrap();
        assert_eq!(strategy, ReleaseStrategyName::SemverCascade);
        assert_eq!(
            next_version(strategy, &Version::new(1, 2, 3)).unwrap(),
            Version::new(1, 2, 4)
        );
    }

    #[test]
    fn semver_cascade_finalizes_a_prerelease() {
        // A prerelease promotes to its release rather than dropping into the
        // next patch (which would silently discard the prerelease train).
        assert_eq!(
            next_version(ReleaseStrategyName::SemverCascade, &parse("1.0.0-rc.1")).unwrap(),
            Version::new(1, 0, 0)
        );
    }

    #[test]
    fn caret_prerelease_advances_the_train_but_patch_bumps_stable() {
        // Stable input: same patch bump as semver-cascade.
        assert_eq!(
            next_version(ReleaseStrategyName::CaretPrerelease, &Version::new(1, 2, 3)).unwrap(),
            Version::new(1, 2, 4)
        );
        // Prerelease input: keep the train together, distinct from semver-cascade.
        assert_eq!(
            next_version(ReleaseStrategyName::CaretPrerelease, &parse("1.0.0-rc.1")).unwrap(),
            parse("1.0.0-rc.2")
        );
        assert_eq!(
            next_version(
                ReleaseStrategyName::CaretPrerelease,
                &parse("2.0.0-alpha.3")
            )
            .unwrap(),
            parse("2.0.0-alpha.4")
        );
        // A train with no numeric tail starts one.
        assert_eq!(
            next_version(ReleaseStrategyName::CaretPrerelease, &parse("1.0.0-rc")).unwrap(),
            parse("1.0.0-rc.1")
        );
    }
}
