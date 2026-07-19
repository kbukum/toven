//! Dependency edges between modules, as a separate typed list.

use serde::{Deserialize, Serialize};

use crate::identity::ModuleKey;

/// Kind of dependency an [`Edge`] represents.
///
/// `kind` matters for affected-filtering: a `Dev` (test-only) change affects
/// tests but not downstream builds, while `Overlay` carries cross-ecosystem
/// edges that native metadata cannot prove.
#[derive(Debug, Clone, Copy, Eq, Ord, PartialEq, PartialOrd, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DepKind {
    /// Normal build/runtime dependency.
    Normal,
    /// Test-only dependency (e.g. Cargo dev-dependencies).
    Dev,
    /// Build-time dependency (e.g. build scripts).
    Build,
    /// Cross-ecosystem edge declared via config overlay.
    Overlay,
}

/// A directed dependency edge: `from` depends on `to`.
///
/// Endpoints are [`ModuleKey`]s so a cross-repo umbrella can carry the same
/// `ecosystem:name` exposed by two members as distinct edges; an intra-repo
/// edge constructed from bare [`ModuleRef`](crate::ModuleRef)s stays
/// member-unscoped.
#[derive(Debug, Clone, Eq, Ord, PartialEq, PartialOrd, Hash, Deserialize, Serialize)]
pub struct Edge {
    /// Module that depends on `to`.
    pub from: ModuleKey,
    /// Module required by `from`.
    pub to: ModuleKey,
    /// Relationship kind.
    pub kind: DepKind,
}

impl Edge {
    /// Construct an edge between two module keys.
    ///
    /// Accepts anything convertible into a [`ModuleKey`], so an intra-repo edge
    /// built from bare [`ModuleRef`](crate::ModuleRef)s reads unchanged.
    pub fn new(from: impl Into<ModuleKey>, to: impl Into<ModuleKey>, kind: DepKind) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            kind,
        }
    }
}
