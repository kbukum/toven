//! The discovery response — one ecosystem's modules, edges, and workspaces.

use serde::{Deserialize, Serialize};
use toven_model::{EcosystemId, Edge, Module, Workspace};

use super::request::DISCOVERY_SCHEMA_VERSION;

/// One ecosystem's contribution to the federated graph.
///
/// Federation is a plain union of these across loaded ecosystems:
/// `⋃ workspaces`, `⋃ modules`, `⋃ edges` (+ config overlay edges). Each
/// `module.workspace` references a [`Workspace::id`] in `workspaces`.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
pub struct DiscoverResponse {
    /// Envelope schema version ([`DISCOVERY_SCHEMA_VERSION`](super::DISCOVERY_SCHEMA_VERSION)).
    pub schema_version: u16,
    /// Ecosystem that produced this response.
    pub ecosystem: EcosystemId,
    /// Resolved discovery units (each carrying its toolchain driver).
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
    /// Discovered modules; `module.workspace` links to a `workspaces` entry.
    #[serde(default)]
    pub modules: Vec<Module>,
    /// Intra-ecosystem dependency edges.
    #[serde(default)]
    pub edges: Vec<Edge>,
    /// Non-fatal warnings (e.g. an unresolved optional dependency).
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl DiscoverResponse {
    /// Construct an empty response for `ecosystem`, stamped with the current
    /// schema version. Adapters push modules/edges/workspaces directly.
    #[must_use]
    pub const fn new(ecosystem: EcosystemId) -> Self {
        Self {
            schema_version: DISCOVERY_SCHEMA_VERSION,
            ecosystem,
            workspaces: Vec::new(),
            modules: Vec::new(),
            edges: Vec::new(),
            warnings: Vec::new(),
        }
    }
}
