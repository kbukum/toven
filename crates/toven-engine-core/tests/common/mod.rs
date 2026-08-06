#![allow(dead_code, clippy::redundant_pub_crate)]
//! Shared helpers for the `toven-engine` config integration tests.

use std::collections::BTreeSet;

use toven_engine_core::config::CanonicalRegistry;
use toven_model::EcosystemId;

/// Construct a validated [`EcosystemId`] for a test id.
pub(crate) fn eid(id: &str) -> EcosystemId {
    EcosystemId::new(id).expect("test ecosystem id is valid")
}

/// Build the loaded-provider id set from string ids.
pub(crate) fn loaded(ids: &[&str]) -> BTreeSet<EcosystemId> {
    ids.iter().map(|id| eid(id)).collect()
}

/// The canonical registry embedded in `toven-model`.
pub(crate) fn canonical() -> CanonicalRegistry {
    CanonicalRegistry::model()
}
