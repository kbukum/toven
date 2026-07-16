//! Bump-policy vocabulary: the default bump [`BumpLevel`] and the
//! [`DependentVersion`] cascade behavior.

use serde::{Deserialize, Serialize};

/// The default version-bump level a release applies to a changed module.
///
/// The per-run bump argv layers over this config default; `Auto`
/// defers the level to the change classification (patch unless a breaking signal
/// forces minor). An **engine-owned** policy value: the adapter only carries the
/// user's selection through to the bump planner.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum BumpLevel {
    /// Bump the patch component (`1.2.3` → `1.2.4`).
    Patch,
    /// Bump the minor component and zero the patch (`1.2.3` → `1.3.0`).
    Minor,
    /// Bump the major component and zero minor/patch (`1.2.3` → `2.0.0`).
    Major,
    /// Defer to the change classification: patch unless a breaking signal forces
    /// a minor bump.
    Auto,
}

impl BumpLevel {
    /// Canonical config/report name for the level.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Patch => "patch",
            Self::Minor => "minor",
            Self::Major => "major",
            Self::Auto => "auto",
        }
    }
}

/// How a dependency-floor bump cascades into a module's dependents.
///
/// When a released module raises a dependency floor in a dependent's manifest,
/// this decides whether the dependent itself receives an own-version bump.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum DependentVersion {
    /// Bump the dependent's own version alongside the floor update (the default
    /// cascade — a dependent that pins a released dependency is itself released).
    Bump,
    /// Only raise the dependency floor; leave the dependent's own version
    /// unchanged (it is not re-released just because a dependency moved).
    Upgrade,
}

impl DependentVersion {
    /// Canonical config/report name for the cascade behavior.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bump => "bump",
            Self::Upgrade => "upgrade",
        }
    }
}
