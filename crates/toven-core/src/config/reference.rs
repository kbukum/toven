//! `ecosystem:module` reference grammar — structural validity only.
//!
//! This is the *structural* half of the two-phase ref contract: a ref is
//! checked for well-formed syntax and a canonical ecosystem prefix here, at
//! load time. Whether the ref resolves to a real module (and whether the
//! resulting graph is acyclic) is the *semantic* half, deferred to the engine
//! Graph phase.

use rskit_errors::{AppError, AppResult};
use toven_model::{EcosystemId, ModuleRef};

use super::CanonicalRegistry;

/// Structural validator for the `ecosystem:module` reference grammar.
#[derive(Debug, Clone, Copy)]
pub struct ModuleRefSyntax;

impl ModuleRefSyntax {
    /// Validate a group `modules` entry.
    ///
    /// A qualified `ecosystem:module` entry is checked in full; a bare entry is
    /// only legal when the group declares a default `ecosystem`, and its name
    /// must be a path-safe identifier.
    pub fn validate_membership(
        field: &str,
        entry: &str,
        default_ecosystem: Option<&EcosystemId>,
        canonical: &CanonicalRegistry,
    ) -> AppResult<()> {
        if entry.contains(':') {
            return Self::validate_qualified(field, entry, canonical);
        }
        if default_ecosystem.is_none() {
            return Err(AppError::invalid_input(
                field,
                format!(
                    "bare module '{entry}' needs a group 'ecosystem' default or an 'ecosystem:module' qualifier"
                ),
            ));
        }
        rskit_validation::input::validate_path_safe_identifier(field, entry)
    }

    /// Validate a fully-qualified `ecosystem:module` reference.
    ///
    /// The ecosystem prefix must be canonical (a typo prefix is rejected here,
    /// not deferred) and the module name must be a path-safe identifier.
    pub fn validate_qualified(
        field: &str,
        value: &str,
        canonical: &CanonicalRegistry,
    ) -> AppResult<()> {
        let reference = ModuleRef::parse(value).map_err(|error| {
            AppError::invalid_input(field, format!("malformed reference '{value}': {error}"))
        })?;
        if !canonical.contains(&reference.ecosystem) {
            return Err(AppError::invalid_input(
                field,
                format!(
                    "reference '{value}' names unknown ecosystem '{}'",
                    reference.ecosystem
                ),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{CanonicalRegistry, ModuleRefSyntax};
    use toven_model::EcosystemId;

    fn registry() -> CanonicalRegistry {
        CanonicalRegistry::model()
    }

    fn rust() -> EcosystemId {
        EcosystemId::new("rust").unwrap()
    }

    #[test]
    fn qualified_ref_with_canonical_prefix_is_accepted() {
        assert!(ModuleRefSyntax::validate_qualified("f", "rust:errors", &registry()).is_ok());
    }

    #[test]
    fn qualified_ref_with_extra_separator_is_rejected() {
        assert!(ModuleRefSyntax::validate_qualified("f", "rust:bad:ref", &registry()).is_err());
    }

    #[test]
    fn qualified_ref_with_unknown_prefix_is_rejected() {
        assert!(ModuleRefSyntax::validate_qualified("f", "rsut:errors", &registry()).is_err());
    }

    #[test]
    fn bare_membership_needs_a_group_default_ecosystem() {
        let canonical = registry();
        assert!(
            ModuleRefSyntax::validate_membership("f", "errors", Some(&rust()), &canonical).is_ok()
        );
        assert!(ModuleRefSyntax::validate_membership("f", "errors", None, &canonical).is_err());
    }

    #[test]
    fn qualified_membership_ignores_the_group_default() {
        assert!(
            ModuleRefSyntax::validate_membership("f", "go:api", Some(&rust()), &registry()).is_ok()
        );
    }
}
