//! The semver-increment matrix.
//!
//! Given a resolved [`EffectiveLevel`] and an optional prerelease channel,
//! [`next_version`] computes the next version. Without a channel it advances
//! the requested component (finalizing a pending prerelease on a patch); with a
//! channel it starts or continues a prerelease train. The bump *decision* (which
//! level, which channel) is made by the caller — this module is pure math.

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_version::semver::Version;

/// A concrete, `Auto`-resolved bump level: the semver component to advance.
///
/// A caller resolves any `Auto`/breaking-change signal into one of these before
/// reaching the matrix, so the version math never has to guess a level.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum EffectiveLevel {
    /// Advance the patch component (`1.2.3` → `1.2.4`).
    Patch,
    /// Advance the minor component, zeroing patch (`1.2.3` → `1.3.0`).
    Minor,
    /// Advance the major component, zeroing minor/patch (`1.2.3` → `2.0.0`).
    Major,
}

/// Compute the next version for `current`, applying `level` and an optional
/// prerelease `channel`.
///
/// Without a channel, the matrix advances the requested semver component; a
/// patch bump of a pending prerelease finalizes it to its release (`1.2.0-rc.1`
/// → `1.2.0`). With a channel, the target sits on a prerelease train:
/// continuing the same channel on the same base increments its numeric tail
/// (`1.2.4-rc.1` → `1.2.4-rc.2`), otherwise a fresh train starts at `.1` on the
/// bumped base (`1.2.3` + patch + `rc` → `1.2.4-rc.1`).
///
/// # Errors
/// Returns an error only if composing a prerelease fails to parse as a valid
/// semantic version (not expected for the bounded channel/level inputs).
pub fn next_version(
    current: &Version,
    level: EffectiveLevel,
    channel: Option<&str>,
) -> AppResult<Version> {
    channel.map_or_else(
        || Ok(stable_bump(current, level)),
        |channel| prerelease_bump(current, level, channel),
    )
}

/// The stable target for `current` at `level`, dropping any prerelease/build.
fn stable_bump(current: &Version, level: EffectiveLevel) -> Version {
    match level {
        EffectiveLevel::Major => Version::new(current.major.saturating_add(1), 0, 0),
        EffectiveLevel::Minor => Version::new(current.major, current.minor.saturating_add(1), 0),
        EffectiveLevel::Patch => {
            if current.pre.is_empty() {
                Version::new(
                    current.major,
                    current.minor,
                    current.patch.saturating_add(1),
                )
            } else {
                // A pending prerelease finalizes to its release rather than silently discarding
                // the train into the next patch.
                Version::new(current.major, current.minor, current.patch)
            }
        }
    }
}

/// The prerelease target for `current` at `level` on `channel`.
fn prerelease_bump(current: &Version, level: EffectiveLevel, channel: &str) -> AppResult<Version> {
    let base = stable_bump(current, level);
    let continuing = !current.pre.is_empty()
        && channel_matches(current.pre.as_str(), channel)
        && base.major == current.major
        && base.minor == current.minor
        && base.patch == current.patch;
    let next = if continuing {
        trailing_number(current.pre.as_str()).saturating_add(1)
    } else {
        1
    };
    let raw = format!(
        "{}.{}.{}-{channel}.{next}",
        base.major, base.minor, base.patch
    );
    Version::parse(&raw).map_err(|error| {
        AppError::new(
            ErrorCode::InvalidFormat,
            format!("failed to compose prerelease '{raw}' for '{current}'"),
        )
        .with_cause(error)
    })
}

/// Whether a prerelease identifier belongs to `channel` (its leading
/// dot-separated segment equals the channel).
fn channel_matches(pre: &str, channel: &str) -> bool {
    pre.split('.').next() == Some(channel)
}

/// The trailing dot-separated numeric identifier of a prerelease train, or `0`
/// when it has no numeric tail (so `rc` → `1`, `rc.1` → `2`).
fn trailing_number(pre: &str) -> u64 {
    pre.rsplit('.')
        .next()
        .and_then(|tail| tail.parse::<u64>().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use rskit_version::semver::Version;

    use super::{EffectiveLevel, next_version};

    fn parse(raw: &str) -> Version {
        Version::parse(raw).unwrap()
    }

    #[test]
    fn stable_matrix_advances_the_requested_component() {
        let current = Version::new(1, 2, 3);
        assert_eq!(
            next_version(&current, EffectiveLevel::Patch, None).unwrap(),
            Version::new(1, 2, 4)
        );
        assert_eq!(
            next_version(&current, EffectiveLevel::Minor, None).unwrap(),
            Version::new(1, 3, 0)
        );
        assert_eq!(
            next_version(&current, EffectiveLevel::Major, None).unwrap(),
            Version::new(2, 0, 0)
        );
    }

    #[test]
    fn patch_of_a_pending_prerelease_finalizes_it() {
        assert_eq!(
            next_version(&parse("1.0.0-rc.1"), EffectiveLevel::Patch, None).unwrap(),
            Version::new(1, 0, 0)
        );
    }

    #[test]
    fn prerelease_channel_starts_and_continues_a_train() {
        // Stable + patch + rc starts a fresh train on the bumped base.
        assert_eq!(
            next_version(&Version::new(1, 2, 3), EffectiveLevel::Patch, Some("rc")).unwrap(),
            parse("1.2.4-rc.1")
        );
        // Same channel on the same base increments the numeric tail.
        assert_eq!(
            next_version(&parse("1.2.4-rc.1"), EffectiveLevel::Patch, Some("rc")).unwrap(),
            parse("1.2.4-rc.2")
        );
        // A different channel restarts the train at `.1`.
        assert_eq!(
            next_version(&parse("1.2.4-rc.1"), EffectiveLevel::Patch, Some("beta")).unwrap(),
            parse("1.2.4-beta.1")
        );
        // A higher level moves the base and restarts the train.
        assert_eq!(
            next_version(&parse("1.2.4-rc.1"), EffectiveLevel::Minor, Some("rc")).unwrap(),
            parse("1.3.0-rc.1")
        );
    }
}
