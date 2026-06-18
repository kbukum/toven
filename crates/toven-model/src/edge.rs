//! Dependency edges between modules, as a separate typed list.

use serde::{Deserialize, Serialize};

use crate::identity::ModuleRef;

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
#[derive(Debug, Clone, Eq, Ord, PartialEq, PartialOrd, Hash, Deserialize, Serialize)]
pub struct Edge {
    /// Module that depends on `to`.
    pub from: ModuleRef,
    /// Module required by `from`.
    pub to: ModuleRef,
    /// Relationship kind.
    pub kind: DepKind,
}

impl Edge {
    /// Construct an edge.
    #[must_use]
    pub const fn new(from: ModuleRef, to: ModuleRef, kind: DepKind) -> Self {
        Self { from, to, kind }
    }
}
