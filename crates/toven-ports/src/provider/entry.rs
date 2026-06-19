//! The stateless registered entry point: id, config baking, and scaffolding.

use std::path::Path;

use rskit_errors::AppResult;
use toven_model::EcosystemId;

use super::{ConfiguredAdapter, EcosystemFragment};

/// The stateless, id-registered entry point for an ecosystem.
///
/// Held by the loaded-provider registry as `dyn Provider`. Object-safe: it bakes
/// a raw `[ecosystems.<id>]` subtree into a config-bearing
/// [`ConfiguredAdapter`], and (config-less) self-detects + scaffolds its section.
pub trait Provider {
    /// The ecosystem this provider serves (`[ecosystems.<id>]` key).
    fn ecosystem_id(&self) -> &EcosystemId;

    /// Parse + bake the raw `[ecosystems.<id>]` subtree into a configured adapter.
    ///
    /// The adapter deserializes `raw` into its own typed schema (with
    /// [`CommonEcosystemConfig`](crate::config::CommonEcosystemConfig) flattened)
    /// and applies defaults. Strict unknown-key rejection for the flattened
    /// section is the `Document` loader's job, since serde cannot combine
    /// `deny_unknown_fields` with `#[serde(flatten)]`.
    fn configure(&self, raw: toml::Value) -> AppResult<Box<dyn ConfiguredAdapter>>;

    /// Config-less detection: emit a minimal `[ecosystems.<id>]` fragment if this
    /// ecosystem applies under `project_root`. `None` = not present.
    fn scaffold(&self, project_root: &Path) -> AppResult<Option<EcosystemFragment>>;
}
