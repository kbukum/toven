//! Release visibility: who a published release is exposed to.
//!
//! [`Visibility`] is the typed exposure a release is cut with — `public`,
//! `private`, or `internal` — resolved from the `[…release].visibility` config
//! field and carried on the resolved release model. It is **enforced fail-closed
//! at the registry-publish boundary**: a non-public release aimed at a
//! public-only registry (crates.io) is rejected at plan time and again by the
//! registry adapter as a last line of defense. The tag push and the hosted forge
//! Release are visibility-agnostic — their exposure follows the remote
//! repository, which Toven does not own — so visibility is recorded intent, not
//! a per-Release forge flag. The default is `public`: a module that omits
//! `visibility` releases exactly as it does today.

use serde::{Deserialize, Serialize};

/// Who a published release is exposed to.
///
/// A first-class, typed field rather than a forge-specific flag. It is enforced
/// where a target can actually violate it — the registry publish — while the tag
/// push and hosted forge Release follow the remote repository's own exposure.
/// `#[non_exhaustive]` because forges may grow further exposure tiers.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Visibility {
    /// Exposed to everyone — the default. A public repository, a public
    /// registry version, and a public hosted Release.
    #[default]
    Public,
    /// Restricted to explicitly authorized principals. Requires a target that
    /// can create a private repository/registry/Release; a public-only target
    /// fails closed.
    Private,
    /// Restricted to the owning organization. Requires a target that can create
    /// an internal repository/registry/Release; a public-only target fails
    /// closed.
    Internal,
}

impl Visibility {
    /// Canonical config/report name for the visibility.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Private => "private",
            Self::Internal => "internal",
        }
    }

    /// Whether this exposure is public (the widest, default tier).
    ///
    /// A public-only target (e.g. crates.io) can honor exactly this exposure and
    /// fails closed on any other.
    #[must_use]
    pub const fn is_public(self) -> bool {
        matches!(self, Self::Public)
    }
}

#[cfg(test)]
mod tests {
    use super::Visibility;

    #[test]
    fn default_is_public() {
        assert_eq!(Visibility::default(), Visibility::Public);
        assert!(Visibility::Public.is_public());
        assert!(!Visibility::Private.is_public());
        assert!(!Visibility::Internal.is_public());
    }

    #[test]
    fn names_are_stable() {
        assert_eq!(Visibility::Public.as_str(), "public");
        assert_eq!(Visibility::Private.as_str(), "private");
        assert_eq!(Visibility::Internal.as_str(), "internal");
    }

    #[test]
    fn parses_known_values_and_rejects_unknown() {
        #[derive(Debug, serde::Deserialize)]
        struct Wrapper {
            visibility: Visibility,
        }

        for (raw, expected) in [
            ("public", Visibility::Public),
            ("private", Visibility::Private),
            ("internal", Visibility::Internal),
        ] {
            let parsed: Wrapper =
                toml::from_str(&format!("visibility = \"{raw}\"")).expect("known value parses");
            assert_eq!(parsed.visibility, expected);
        }

        // No sentinel strings: an unknown value is a typed parse error, never a
        // silently-coerced default.
        let error = toml::from_str::<Wrapper>("visibility = \"secret\"")
            .expect_err("unknown visibility rejected");
        assert!(error.to_string().contains("visibility"), "{error}");
    }

    #[test]
    fn round_trips_through_serde() {
        #[derive(Debug, serde::Serialize, serde::Deserialize)]
        struct Wrapper {
            visibility: Visibility,
        }

        for visibility in [
            Visibility::Public,
            Visibility::Private,
            Visibility::Internal,
        ] {
            let toml = toml::to_string(&Wrapper { visibility }).expect("serializes");
            let back: Wrapper = toml::from_str(&toml).expect("round-trips");
            assert_eq!(back.visibility, visibility);
        }
    }
}
