//! The typed publication policy — how a module participates in registry
//! publication during a release.
//!
//! [`PublicationPolicy`] is resolved from the declarative `[…release]` fields
//! (`registry`, `publish`, `exclude`) with **no sentinel strings**: a present
//! `registry` selects [`PublicationPolicy::Registry`], `publish = false`
//! selects [`PublicationPolicy::TagOnly`], and `exclude = true` selects
//! [`PublicationPolicy::Excluded`]. Contradictory combinations are rejected
//! during configuration validation rather than silently coerced.

use rskit_errors::{AppError, AppResult};

/// How a module participates in registry publication during a release.
///
/// The three states are mutually exclusive and cover the whole release
/// spectrum: a registry-published module, a module that is versioned and tagged
/// but never published to a package registry, and a module excluded from the
/// release entirely.
#[derive(Debug, Clone, Eq, PartialEq)]
#[non_exhaustive]
pub enum PublicationPolicy {
    /// The module is versioned, tagged, packaged, and published to the named
    /// registry, and may cut a hosted release.
    Registry {
        /// Target registry identifier (e.g. `crates-io`).
        registry: String,
    },
    /// The module is versioned and tagged, and may cut a hosted release, but is
    /// never published to a package registry.
    TagOnly,
    /// The module is excluded from the release entirely: no version change, no
    /// tag, no ecosystem target call, and no hosted release.
    Excluded,
}

impl PublicationPolicy {
    /// Resolve the typed policy from the declarative release fields.
    ///
    /// Precedence mirrors the field semantics: an explicit `exclude = true`
    /// wins (the module is dropped from the release), a present `registry`
    /// selects registry publication, and everything else is tag-only. This
    /// assumes configuration validation has already rejected contradictory
    /// combinations (see [`validate_fields`](Self::validate_fields)).
    #[must_use]
    pub fn resolve(registry: Option<&str>, publish: Option<bool>, exclude: bool) -> Self {
        if exclude {
            Self::Excluded
        } else if publish == Some(false) {
            Self::TagOnly
        } else if let Some(registry) = registry {
            Self::Registry {
                registry: registry.to_string(),
            }
        } else {
            Self::TagOnly
        }
    }

    /// Canonical config/report name for the policy.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Registry { .. } => "registry",
            Self::TagOnly => "tag-only",
            Self::Excluded => "excluded",
        }
    }

    /// The target registry identifier when this policy publishes to one.
    #[must_use]
    pub fn registry(&self) -> Option<&str> {
        match self {
            Self::Registry { registry } => Some(registry),
            Self::TagOnly | Self::Excluded => None,
        }
    }

    /// Whether the module publishes packaged artifacts to a package registry.
    #[must_use]
    pub const fn publishes_to_registry(&self) -> bool {
        matches!(self, Self::Registry { .. })
    }

    /// Whether the module participates in the release at all (is not excluded).
    #[must_use]
    pub const fn releases(&self) -> bool {
        !matches!(self, Self::Excluded)
    }

    /// Reject contradictory `registry`/`publish`/`exclude` field combinations.
    ///
    /// `field` is the config path prefix used in diagnostics (e.g.
    /// `ecosystems.rust.release`). This covers only the contradictions
    /// expressible from these three fields alone; ecosystem-specific rules (a
    /// Go registry) and cross-field rules (an excluded module with hosted
    /// assets) are enforced by their respective owners.
    ///
    /// # Errors
    /// Rejects a registry target combined with `publish = false`, and an
    /// excluded module that also declares a registry or requests publication.
    pub fn validate_fields(
        field: &str,
        registry: Option<&str>,
        publish: Option<bool>,
        exclude: Option<bool>,
    ) -> AppResult<()> {
        let excluded = exclude.unwrap_or(false);
        if registry.is_some() && publish == Some(false) {
            return Err(AppError::invalid_input(
                format!("{field}.publish"),
                "a registry target cannot be combined with publish = false; drop the registry for \
                 a tag-only release or remove publish = false to publish to it",
            ));
        }
        if excluded && registry.is_some() {
            return Err(AppError::invalid_input(
                format!("{field}.exclude"),
                "an excluded module cannot declare a registry; remove the registry or set exclude \
                 = false",
            ));
        }
        if excluded && publish == Some(true) {
            return Err(AppError::invalid_input(
                format!("{field}.exclude"),
                "an excluded module cannot request publish = true; remove exclude = true to \
                 release the module",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::PublicationPolicy;

    #[test]
    fn resolve_maps_fields_to_the_three_states() {
        assert_eq!(
            PublicationPolicy::resolve(Some("crates-io"), None, false),
            PublicationPolicy::Registry {
                registry: "crates-io".to_string()
            }
        );
        assert_eq!(
            PublicationPolicy::resolve(None, Some(false), false),
            PublicationPolicy::TagOnly
        );
        // Exclusion wins even when a registry is present (validation rejects that
        // combination first, but resolution is total).
        assert_eq!(
            PublicationPolicy::resolve(Some("crates-io"), None, true),
            PublicationPolicy::Excluded
        );
    }

    #[test]
    fn accessors_report_the_policy_shape() {
        let registry = PublicationPolicy::resolve(Some("crates-io"), None, false);
        assert_eq!(registry.as_str(), "registry");
        assert_eq!(registry.registry(), Some("crates-io"));
        assert!(registry.publishes_to_registry());
        assert!(registry.releases());

        let tag_only = PublicationPolicy::TagOnly;
        assert_eq!(tag_only.as_str(), "tag-only");
        assert_eq!(tag_only.registry(), None);
        assert!(!tag_only.publishes_to_registry());
        assert!(tag_only.releases());

        let excluded = PublicationPolicy::Excluded;
        assert_eq!(excluded.as_str(), "excluded");
        assert!(!excluded.publishes_to_registry());
        assert!(!excluded.releases());
    }

    #[test]
    fn validate_rejects_registry_with_publish_false() {
        let error = PublicationPolicy::validate_fields("r", Some("crates-io"), Some(false), None)
            .expect_err("registry + publish=false is contradictory");
        assert!(error.to_string().contains("registry target"));
    }

    #[test]
    fn validate_rejects_excluded_with_registry_or_publish() {
        assert!(
            PublicationPolicy::validate_fields("r", Some("crates-io"), None, Some(true)).is_err()
        );
        assert!(PublicationPolicy::validate_fields("r", None, Some(true), Some(true)).is_err());
    }

    #[test]
    fn validate_accepts_coherent_combinations() {
        PublicationPolicy::validate_fields("r", Some("crates-io"), None, None).expect("registry");
        PublicationPolicy::validate_fields("r", None, Some(false), None).expect("tag-only");
        PublicationPolicy::validate_fields("r", None, None, Some(true)).expect("excluded");
    }
}
