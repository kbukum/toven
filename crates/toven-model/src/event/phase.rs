//! [`Phase`] — the named phases of the pure PLAN half, reported for progress.

use serde::{Deserialize, Serialize};

/// A named phase of the pure PLAN half, reported for progress.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// Parse + structurally validate config.
    Load,
    /// Configure per-ecosystem adapters.
    Configure,
    /// Federated discovery across loaded ecosystems.
    Discover,
    /// Build + validate the dependency graph.
    Graph,
    /// Map changes to the active module set.
    Affected,
    /// Resolve toolchain identity for active workspaces.
    Toolchain,
    /// Compute the federated wave sequence + cache verdicts.
    Schedule,
}
