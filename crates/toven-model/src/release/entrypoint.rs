//! [`Entrypoint`] — who cuts the release the flow runs against.

use serde::{Deserialize, Serialize};

/// Who cuts the release the flow runs against — the flow's *entrypoint*.
///
/// A release flow is the same ordered set of phases either way, but the actor
/// that creates the tag and the hosted Release differs:
///
/// - [`Toven`](Self::Toven) — the default. Toven owns the whole flow end to
///   end: it decides the version, cuts the tag, publishes, and creates the
///   hosted Release. The [`Tag`](crate::ReleasePhase::Tag) phase is a
///   *mutation* Toven performs.
/// - [`Maintainer`](Self::Maintainer) — a human creates the tag and the hosted
///   Release in the forge UI (the `release: published` flow), and Toven then
///   runs **against that existing tag/Release**: it verifies the tag matches
///   the planned version (never creating or moving it), publishes, attaches
///   assets through the host adapter's create-or-verify path, and attests
///   provenance. In this mode the [`Tag`](crate::ReleasePhase::Tag) phase is an
///   *input*, not a mutation, and no manifest mutation or release commit
///   happens at publish time — the version/CHANGELOG decision already merged
///   through the [`Bump`](crate::ReleasePhase::Bump) phase.
///
/// This is purely descriptive vocabulary: it *names* which actor owns tag and
/// Release creation so config, planning, and reporting can refer to it. The
/// engine owns every phase's flow guarantees (mutation-free preview,
/// immutability with forward-fix recovery, typed reporting) regardless of the
/// entrypoint — a maintainer-owned flow never hands ownership of selection,
/// ordering, readiness, or reporting to anyone.
///
/// `#[non_exhaustive]` because further entrypoints may be modeled later.
#[derive(
    Debug, Clone, Copy, Default, Eq, PartialEq, Ord, PartialOrd, Hash, Deserialize, Serialize,
)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Entrypoint {
    /// Toven owns the whole flow, cutting the tag and the hosted Release
    /// itself — the default.
    #[default]
    Toven,
    /// A maintainer created the tag and hosted Release; Toven runs against that
    /// existing tag/Release, verifying rather than creating it.
    Maintainer,
}

impl Entrypoint {
    /// Every entrypoint, in a stable order.
    pub const ALL: &'static [Self] = &[Self::Toven, Self::Maintainer];

    /// Canonical config/report name for the entrypoint (stable, matches the
    /// serde representation).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Toven => "toven",
            Self::Maintainer => "maintainer",
        }
    }

    /// Whether Toven cuts the tag and hosted Release itself (the default,
    /// Toven-owned flow).
    #[must_use]
    pub const fn is_toven_owned(self) -> bool {
        matches!(self, Self::Toven)
    }

    /// Whether a maintainer already created the tag and hosted Release, so
    /// Toven runs against them as input rather than creating them.
    #[must_use]
    pub const fn is_maintainer_owned(self) -> bool {
        matches!(self, Self::Maintainer)
    }
}

#[cfg(test)]
mod tests {
    use super::Entrypoint;

    #[test]
    fn default_is_toven_owned() {
        assert_eq!(Entrypoint::default(), Entrypoint::Toven);
        assert!(Entrypoint::Toven.is_toven_owned());
        assert!(!Entrypoint::Toven.is_maintainer_owned());
        assert!(Entrypoint::Maintainer.is_maintainer_owned());
        assert!(!Entrypoint::Maintainer.is_toven_owned());
    }

    #[test]
    fn names_are_stable() {
        assert_eq!(Entrypoint::Toven.as_str(), "toven");
        assert_eq!(Entrypoint::Maintainer.as_str(), "maintainer");
    }

    #[test]
    fn all_lists_every_entrypoint() {
        let names: Vec<&str> = Entrypoint::ALL.iter().map(|e| e.as_str()).collect();
        assert_eq!(names, ["toven", "maintainer"]);
    }

    #[test]
    fn parses_known_values_and_rejects_unknown() {
        for (raw, expected) in [
            ("toven", Entrypoint::Toven),
            ("maintainer", Entrypoint::Maintainer),
        ] {
            let parsed: Entrypoint =
                serde_json::from_str(&format!("\"{raw}\"")).expect("known value parses");
            assert_eq!(parsed, expected);
        }

        // No sentinel strings: an unknown value is a typed parse error, never a
        // silently-coerced default.
        let error =
            serde_json::from_str::<Entrypoint>("\"ci\"").expect_err("unknown entrypoint rejected");
        assert!(error.to_string().contains("ci"), "{error}");
    }

    #[test]
    fn round_trips_through_serde() {
        for entrypoint in Entrypoint::ALL.iter().copied() {
            let json = serde_json::to_string(&entrypoint).expect("serializes");
            assert_eq!(json, format!("\"{}\"", entrypoint.as_str()));
            let back: Entrypoint = serde_json::from_str(&json).expect("round-trips");
            assert_eq!(back, entrypoint);
        }
    }
}
