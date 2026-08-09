//! Comparison-endpoint vocabulary — the two addressable sides of a diff.
//!
//! Pure vocabulary: a [`DiffEndpoint`] names *where* one side of a comparison
//! lives (the working tree, `HEAD`, a named ref, an object id, or the latest
//! tag matching a scheme) without performing any git call. The engine-core
//! change foundation resolves a [`DiffRange`] of two endpoints onto the
//! [`VcsReader`](super::VcsReader) seam, so every verb expresses "what changed
//! between two points" the same way.

use serde::{Deserialize, Serialize};

use crate::TagScheme;

use super::Oid;

/// One addressable side of a comparison.
///
/// Kept git-call-free: resolution (listing tags, selecting the latest match,
/// diffing) is the engine's job, not this value's.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum DiffEndpoint {
    /// The uncommitted working tree (committed changes ∪ working-tree status).
    WorkingTree,
    /// The `HEAD` commit.
    Head,
    /// A ref by name — a branch, tag, or commit-ish resolved verbatim.
    Ref(String),
    /// A resolved object id.
    Oid(Oid),
    /// The latest tag whose name matches `scheme`, by max semver.
    LatestTag {
        /// The tag grammar the latest match is selected through.
        scheme: TagScheme,
    },
}

impl DiffEndpoint {
    /// A ref endpoint from any string-like ref name.
    #[must_use]
    pub fn reference(name: impl Into<String>) -> Self {
        Self::Ref(name.into())
    }

    /// A latest-matching-tag endpoint for `scheme`.
    #[must_use]
    pub const fn latest_tag(scheme: TagScheme) -> Self {
        Self::LatestTag { scheme }
    }
}

/// A comparison between two endpoints: what changed going `from` → `to`.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
pub struct DiffRange {
    /// The baseline side of the comparison.
    pub from: DiffEndpoint,
    /// The target side of the comparison.
    pub to: DiffEndpoint,
}

impl DiffRange {
    /// Construct a range from a baseline `from` to a target `to`.
    #[must_use]
    pub const fn new(from: DiffEndpoint, to: DiffEndpoint) -> Self {
        Self { from, to }
    }
}

#[cfg(test)]
mod tests {
    use crate::TagScheme;

    use super::{DiffEndpoint, DiffRange};

    #[test]
    fn reference_wraps_a_ref_name() {
        assert_eq!(
            DiffEndpoint::reference("main"),
            DiffEndpoint::Ref("main".to_string())
        );
    }

    #[test]
    fn latest_tag_carries_its_scheme() {
        let scheme = TagScheme::new("rust/core@", "");
        assert_eq!(
            DiffEndpoint::latest_tag(scheme.clone()),
            DiffEndpoint::LatestTag { scheme }
        );
    }

    #[test]
    fn range_round_trips_through_toml() {
        let range = DiffRange::new(DiffEndpoint::reference("v1.0.0"), DiffEndpoint::WorkingTree);
        let serialized = toml::to_string(&range).expect("serialize");
        let back: DiffRange = toml::from_str(&serialized).expect("deserialize");
        assert_eq!(range, back);
    }
}
