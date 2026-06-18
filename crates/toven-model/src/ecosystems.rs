//! Canonical ecosystem registry.
//!
//! A static, curated list of known [`EcosystemId`](crate::EcosystemId)s embedded
//! in *every* binary, independent of which adapters are linked. It exists so a
//! standalone tool can tell a legitimate-but-unloaded ecosystem (warn + ignore)
//! apart from a typo (hard error) — the central known-ecosystem registry from
//! architecture §4. Adding an ecosystem is one line here plus shipping its adapter.
//!
//! This is immutable data looked up by value, not a mutable global registry.

/// A canonical ecosystem: its id plus a human-readable label.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct CanonicalEcosystem {
    /// Canonical identifier (e.g. `rust`).
    pub id: &'static str,
    /// Human-readable label used in diagnostics (e.g. `Rust`).
    pub label: &'static str,
}

/// The curated set of canonical ecosystems.
const CANONICAL: &[CanonicalEcosystem] = &[
    CanonicalEcosystem {
        id: "rust",
        label: "Rust",
    },
    CanonicalEcosystem {
        id: "go",
        label: "Go",
    },
    CanonicalEcosystem {
        id: "ts",
        label: "TypeScript",
    },
    CanonicalEcosystem {
        id: "python",
        label: "Python",
    },
];

/// All canonical ecosystems, in registry order.
#[must_use]
pub const fn canonical_ecosystems() -> &'static [CanonicalEcosystem] {
    CANONICAL
}

/// Whether `id` is a canonical (known) ecosystem.
#[must_use]
pub fn is_canonical(id: &str) -> bool {
    CANONICAL.iter().any(|entry| entry.id == id)
}

/// Human-readable label for a canonical ecosystem id, if known.
#[must_use]
pub fn label(id: &str) -> Option<&'static str> {
    CANONICAL
        .iter()
        .find(|entry| entry.id == id)
        .map(|entry| entry.label)
}

#[cfg(test)]
mod tests {
    use super::{canonical_ecosystems, is_canonical, label};

    #[test]
    fn known_ids_resolve() {
        assert!(is_canonical("rust"));
        assert!(is_canonical("go"));
        assert_eq!(label("python"), Some("Python"));
    }

    #[test]
    fn typos_are_not_canonical() {
        assert!(!is_canonical("rsut"));
        assert_eq!(label("rsut"), None);
    }

    #[test]
    fn registry_is_non_empty_and_unique() {
        let entries = canonical_ecosystems();
        assert!(!entries.is_empty());
        for (index, entry) in entries.iter().enumerate() {
            assert!(
                !entries[..index].iter().any(|prior| prior.id == entry.id),
                "duplicate canonical id '{}'",
                entry.id
            );
        }
    }
}
