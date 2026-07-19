//! Ecosystem-id three-way dispatch (loaded / canonical-unloaded / unknown).
//!
//! A standalone binary that only links the Rust adapter must still tell a legit
//! but unloaded `[ecosystems.go]` (warn + ignore) apart from a typo
//! `[ecosystems.rsut]` (hard error). It cannot from its loaded adapters alone,
//! so dispatch consults *two* registries: the [`CanonicalRegistry`] of known
//! ids and the set of ids actually loaded in this binary.

use std::collections::{BTreeMap, BTreeSet};

use rskit_config::RawValue;
use rskit_errors::{AppError, AppResult};
use toven_model::EcosystemId;

use super::{CanonicalRegistry, Document};

/// The outcome of dispatching every `[ecosystems.<id>]` section.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct Dispatch {
    /// Sections whose ecosystem is loaded here: the raw subtree handed to the
    /// adapter's `configure`, keyed by ecosystem id.
    pub configurable: BTreeMap<EcosystemId, RawValue>,
    /// Canonical-but-unloaded ecosystems: ignored, surfaced for a CLI warning.
    pub ignored: Vec<EcosystemId>,
}

/// Classify every `[ecosystems.<id>]` section of `document`.
///
/// - id is loaded here → kept in [`Dispatch::configurable`];
/// - else id is canonical → recorded in [`Dispatch::ignored`] (warn + ignore);
/// - else → hard error (a typo or otherwise unknown ecosystem).
pub fn dispatch(
    document: &Document,
    loaded: &BTreeSet<EcosystemId>,
    canonical: &CanonicalRegistry,
) -> AppResult<Dispatch> {
    let mut result = Dispatch::default();
    for (id, raw) in &document.ecosystems {
        if loaded.contains(id) {
            result.configurable.insert(id.clone(), raw.clone());
        } else if canonical.contains(id) {
            result.ignored.push(id.clone());
        } else {
            return Err(AppError::invalid_input(
                "ecosystems",
                format!("unknown ecosystem '{id}' (not a known ecosystem and no adapter loaded)"),
            ));
        }
    }
    Ok(result)
}
