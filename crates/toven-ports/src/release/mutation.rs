//! The atomic version mutation the engine computes and the port applies.

use std::collections::BTreeMap;

use rskit_version::semver::Version;
use toven_model::ModuleRef;

/// One module's atomic release mutation.
///
/// The **engine** computes the full bump plan from the module graph + semver
/// rules; the **port** applies this single mutation, owning all manifest
/// specifics (own version + intra-project dep-floor rewrites). One atomic write
/// per module → no partial-write state on failure.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ReleaseMutation {
    /// The module's new version; `None` leaves it unchanged.
    pub new_version: Option<Version>,
    /// Sibling deps whose declared version floor this manifest must rewrite
    /// (the cascade: when a dep bumps, every dependent re-floors).
    pub dep_floor_updates: BTreeMap<ModuleRef, Version>,
    /// Go import paths whose required versions must be raised in this manifest.
    ///
    /// Ecosystem adapters use this normalized projection when a model-level
    /// dependency reference does not contain the package's import path.
    pub dep_floor_import_updates: BTreeMap<String, Version>,
}

impl ReleaseMutation {
    /// A mutation that only bumps the module's own version.
    #[must_use]
    pub const fn version(new_version: Version) -> Self {
        Self {
            new_version: Some(new_version),
            dep_floor_updates: BTreeMap::new(),
            dep_floor_import_updates: BTreeMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use rskit_version::semver::Version;

    use super::ReleaseMutation;

    #[test]
    fn version_sets_new_version_and_no_dep_floors() {
        let mutation = ReleaseMutation::version(Version::new(1, 2, 3));
        assert_eq!(mutation.new_version, Some(Version::new(1, 2, 3)));
        assert!(mutation.dep_floor_updates.is_empty());
    }
}
