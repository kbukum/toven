//! The top-level strict `Document`.

use std::collections::BTreeMap;

use rskit_config::RawValue;
use toven_model::EcosystemId;

use serde::{Deserialize, Serialize};

use super::{
    GroupConfig, MemberConfig, ModuleConfig, OverlayConfig, ProjectConfig, TovenConfig, VerbId,
};
use toven_ports::HooksConfig;

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
    /// Project-level lifecycle hooks, keyed by the verb they wrap
    /// (`[hooks.<verb>]`). Each value is a `pre`/`post` set of recognized task
    /// references the driver runs around that verb (`pre` fail-closed before the
    /// verb, `post` after it succeeds). An unknown verb key fails the strict
    /// parse; [`Self::hooks_for`] composes umbrella/specific precedence.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub hooks: BTreeMap<VerbId, HooksConfig>,
}

impl Document {
    /// The composed lifecycle hooks for `verb`, honoring umbrella precedence.
    ///
    /// A release mutation (`bump`/`tag`/`publish`) inherits the umbrella
    /// `[hooks.release]` hooks *around* its own: `pre` runs the umbrella's
    /// references first then the specific verb's (specific innermost, closest to
    /// the mutation), and `post` runs the specific verb's first then the
    /// umbrella's (specific innermost again). Every other verb uses only its own
    /// hooks. The result is empty when nothing is configured, so a caller can
    /// skip all hook wiring.
    #[must_use]
    pub fn hooks_for(&self, verb: VerbId) -> HooksConfig {
        let own = self.hooks.get(&verb).cloned().unwrap_or_default();
        let umbrella = match verb {
            VerbId::Bump | VerbId::Tag | VerbId::Publish => self
                .hooks
                .get(&VerbId::Release)
                .cloned()
                .unwrap_or_default(),
            _ => HooksConfig::default(),
        };
        HooksConfig {
            pre: umbrella.pre.into_iter().chain(own.pre).collect(),
            post: own.post.into_iter().chain(umbrella.post).collect(),
        }
    }
}
