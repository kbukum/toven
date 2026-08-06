//! The top-level strict `Document`.

use std::collections::BTreeMap;

use rskit_config::RawValue;
use toven_model::EcosystemId;

use serde::{Deserialize, Serialize};

use super::{GroupConfig, MemberConfig, ModuleConfig, OverlayConfig, ProjectConfig, TovenConfig};

/// The whole `toven.toml`, parsed strictly.
///
/// `#[serde(deny_unknown_fields)]` rejects any stray top-level key for free:
/// `ecosystems` is the only dynamic-keyed field, so a typo like `[porject]` or
/// a stray `[rust]` table fails the parse. Each `[ecosystems.<id>]` subtree is
/// kept verbatim as a [`RawValue`] so the owning adapter parses it later under
/// its own `#[serde(deny_unknown_fields)]` schema (the engine never learns
/// adapter field names).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    /// Repo identity and change baseline.
    pub project: ProjectConfig,
    /// Engine settings (reporting, concurrency, cache, include, drivers).
    #[serde(default)]
    pub toven: TovenConfig,
    /// Federation-level groups, keyed by group name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub groups: BTreeMap<String, GroupConfig>,
    /// Manually declared cross-ecosystem dependency edges.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overlays: Vec<OverlayConfig>,
    /// Raw `[ecosystems.<id>]` subtrees, kept verbatim for adapter `configure`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ecosystems: BTreeMap<EcosystemId, RawValue>,
    /// Per-module release overrides, keyed by `ecosystem:module` reference.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub modules: BTreeMap<String, ModuleConfig>,
    /// Multi-repo umbrella members (single-repo documents leave this empty).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<MemberConfig>,
}
