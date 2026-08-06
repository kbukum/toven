//! The canonical ecosystem-id registry used during dispatch and ref validation.

use std::collections::BTreeSet;

use toven_model::{EcosystemId, canonical_ecosystems};

/// The set of canonical (known) ecosystem ids.
///
/// This is one of the *two* registries the dispatch contract needs: the static,
/// curated list of legitimate ecosystem ids embedded in every binary, distinct
/// from the loaded-provider set actually compiled into *this* binary. Keeping
/// it injectable (rather than reaching straight for the model's static
/// functions) keeps dispatch and structural validation pure and exhaustively
/// testable.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CanonicalRegistry {
    ids: BTreeSet<String>,
}

impl CanonicalRegistry {
    /// Build a registry from the canonical list embedded in [`toven_model`].
    #[must_use]
    pub fn model() -> Self {
        Self {
            ids: canonical_ecosystems()
                .iter()
                .map(|entry| entry.id.to_string())
                .collect(),
        }
    }

    /// Build a registry from an explicit id set (used by tests).
    pub fn new<I, S>(ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            ids: ids.into_iter().map(Into::into).collect(),
        }
    }

    /// Whether `id` is a canonical (known) ecosystem.
    #[must_use]
    pub fn contains(&self, id: &EcosystemId) -> bool {
        self.ids.contains(id.as_str())
    }

    /// Iterate the canonical ecosystem ids as typed [`EcosystemId`]s.
    ///
    /// Used by federation provisioning to enumerate every known ecosystem when
    /// reporting driver status.
    pub fn ids(&self) -> impl Iterator<Item = EcosystemId> + '_ {
        self.ids.iter().filter_map(|id| EcosystemId::new(id).ok())
    }
}

impl Default for CanonicalRegistry {
    fn default() -> Self {
        Self::model()
    }
}

#[cfg(test)]
mod tests {
    use super::CanonicalRegistry;
    use toven_model::EcosystemId;

    #[test]
    fn model_registry_contains_canonical_ids() {
        let registry = CanonicalRegistry::model();
        assert!(registry.contains(&EcosystemId::new("rust").unwrap()));
        assert!(!registry.contains(&EcosystemId::new("rsut").unwrap()));
    }

    #[test]
    fn explicit_registry_uses_only_its_ids() {
        let registry = CanonicalRegistry::new(["rust"]);
        assert!(registry.contains(&EcosystemId::new("rust").unwrap()));
        assert!(!registry.contains(&EcosystemId::new("go").unwrap()));
    }
}
