//! Baseline specification — the typed output of the engine's `BaselineStrategy`.

use serde::{Deserialize, Serialize};

/// How a baseline reference is interpreted.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BaselineMode {
    /// Diff directly against `reference` (`--base`, `[project].base_ref`).
    Explicit,
    /// Diff against `merge-base(reference, HEAD)` (`--merge-base`).
    MergeBase,
}

/// The typed baseline the engine's named `BaselineStrategy` resolves config + CLI
/// flags into.
///
/// The git mechanism ([`VcsReader`](super::VcsReader)) stays policy-free: it
/// consumes this spec via `changed_since`, while the *which-ref / merge-base*
/// decision lives in the engine, pure and unit-testable without a repo.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Deserialize, Serialize)]
pub struct BaselineSpec {
    /// The git reference to baseline against (a ref name, tag, or oid).
    pub reference: String,
    /// How to interpret `reference`.
    pub mode: BaselineMode,
}

impl BaselineSpec {
    /// A spec that diffs directly against `reference`.
    #[must_use]
    pub fn explicit(reference: impl Into<String>) -> Self {
        Self {
            reference: reference.into(),
            mode: BaselineMode::Explicit,
        }
    }

    /// A spec that diffs against `merge-base(reference, HEAD)`.
    #[must_use]
    pub fn merge_base(reference: impl Into<String>) -> Self {
        Self {
            reference: reference.into(),
            mode: BaselineMode::MergeBase,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BaselineMode, BaselineSpec};

    #[test]
    fn explicit_uses_explicit_mode() {
        let spec = BaselineSpec::explicit("main");
        assert_eq!(spec.reference, "main");
        assert_eq!(spec.mode, BaselineMode::Explicit);
    }

    #[test]
    fn merge_base_uses_merge_base_mode() {
        let spec = BaselineSpec::merge_base("origin/main");
        assert_eq!(spec.reference, "origin/main");
        assert_eq!(spec.mode, BaselineMode::MergeBase);
    }

    #[test]
    fn round_trips_through_toml() {
        let spec = BaselineSpec::merge_base("origin/main");
        let json = toml::to_string(&spec).expect("serialize");
        let back: BaselineSpec = toml::from_str(&json).expect("deserialize");
        assert_eq!(spec, back);
    }
}
