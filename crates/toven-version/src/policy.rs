//! Bump-policy vocabulary: the engine-owned named policy plus the winning-input
//! and reason enums a bump decision reports.
//!
//! These are release *policy* names, not semver *mechanism* (which lives in the
//! pure [`toven_semver`] toolkit). They are owned here, beside the decision that
//! produces them, and re-exported by `toven-release` for its public reports.

/// The engine-owned named bump policy.
///
/// The `[…release].strategy` config field resolves to one of these. It selects
/// only the **decide next version** node of the release flow; every other node
/// (change detection, cascade, idempotency, tag/publish) is common to all
/// policies. Prerelease behavior is driven by `--pre <channel>` / the
/// `prerelease` config under [`SemverCascade`](Self::SemverCascade); under
/// [`Manifest`](Self::Manifest) the channel lives in the declared version.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub enum BumpPolicy {
    /// Semantic-version cascade: compute the next version from baseline +
    /// changes — patch by default, minor on a breaking signal, major on
    /// explicit request, finalizing a pending prerelease on a stable bump and
    /// cascading a dependency-floor bump into dependents. Prerelease is driven
    /// only by `--pre`/config. The default.
    SemverCascade,
    /// Manifest-declared: cut exactly the version the manifest declares, when
    /// it is strictly ahead of the last release tag; fail closed otherwise. The
    /// prerelease channel, if any, is part of the declared version.
    Manifest,
}

impl BumpPolicy {
    /// Canonical policy name used by config and reports.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SemverCascade => "semver-cascade",
            Self::Manifest => "manifest",
        }
    }
}

/// Which input decided a module's bump, under the documented precedence (argv >
/// `[modules.<name>.release]` > `[ecosystems.<id>].release` > adapter default).
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub enum BumpSource {
    /// An explicit `--set-version <module>=<x.y.z>` argv override pinned the
    /// target version.
    SetVersion,
    /// An argv level override (`--patch`/`--minor`/`--major <module>`) forced
    /// the level.
    Argv,
    /// The resolved config level (`[modules.<name>.release]` or
    /// `[ecosystems.<id>].release`) selected the level.
    Config,
    /// `Auto` resolved to a minor bump from a breaking changelog
    /// classification.
    Changelog,
    /// `Auto` resolved to the patch default (no breaking signal).
    Default,
    /// A dependency-floor cascade into a dependent that did not itself change.
    Cascade,
    /// The `manifest` bump policy cut the version declared in the manifest.
    Manifest,
}

impl BumpSource {
    /// Canonical report name for the winning input.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SetVersion => "set-version",
            Self::Argv => "argv",
            Self::Config => "config",
            Self::Changelog => "changelog",
            Self::Default => "default",
            Self::Cascade => "cascade",
            Self::Manifest => "manifest",
        }
    }
}

/// Why a module receives a release bump.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub enum BumpReason {
    /// The module itself changed since its release baseline.
    Changed,
    /// The module has never been released, so its declared version is cut as
    /// its first release.
    InitialRelease,
    /// The module bumped only because a dependency's floor rose (cascade).
    DependencyCascade,
    /// The module was pinned to an explicit target version.
    Explicit,
    /// The `manifest` bump policy cut the version declared in the manifest.
    Manifest,
}

impl BumpReason {
    /// Canonical report name for the reason.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Changed => "changed",
            Self::InitialRelease => "initial-release",
            Self::DependencyCascade => "dependency-cascade",
            Self::Explicit => "explicit",
            Self::Manifest => "manifest",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BumpPolicy, BumpReason, BumpSource};

    #[test]
    fn policy_name_is_stable() {
        assert_eq!(BumpPolicy::SemverCascade.as_str(), "semver-cascade");
        assert_eq!(BumpPolicy::Manifest.as_str(), "manifest");
    }

    #[test]
    fn bump_source_and_reason_names_are_stable() {
        assert_eq!(BumpSource::SetVersion.as_str(), "set-version");
        assert_eq!(BumpSource::Cascade.as_str(), "cascade");
        assert_eq!(BumpSource::Manifest.as_str(), "manifest");
        assert_eq!(BumpReason::Changed.as_str(), "changed");
        assert_eq!(BumpReason::DependencyCascade.as_str(), "dependency-cascade");
        assert_eq!(BumpReason::Manifest.as_str(), "manifest");
    }
}
