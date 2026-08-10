//! `[[overlays]]` — cross-ecosystem edges native metadata cannot prove.

use toven_model::EcosystemId;

use serde::{Deserialize, Serialize};

/// A reserved `[[overlays]]` entry: a manually declared dependency edge.
///
/// Used for cross-ecosystem dependencies that no single ecosystem's native
/// manifest can express (e.g. a Go module that consumes a Rust crate). Both
/// endpoints are structured `{ ecosystem, module }` refs.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayConfig {
    /// The dependent endpoint.
    pub from: OverlayRef,
    /// The depended-on endpoint.
    pub to: OverlayRef,
}

/// A structured `{ ecosystem, module }` endpoint of an [`OverlayConfig`].
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayRef {
    /// Owning ecosystem of the referenced module.
    pub ecosystem: EcosystemId,
    /// Module name, unique within its ecosystem.
    pub module: String,
}
