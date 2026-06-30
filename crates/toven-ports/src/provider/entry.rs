//! The stateless registered entry point: id, config baking, and scaffolding.

use std::path::Path;

use rskit_config::RawValue;
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
    /// `raw` is the canonical [`RawValue`] subtree retained verbatim by the
    /// loader (format-neutral, regardless of the on-disk source). The adapter
    /// deserializes it into its own typed schema — typically via
    /// [`rskit_config::deserialize_subtree`] — with
    /// [`CommonEcosystemConfig`](crate::config::CommonEcosystemConfig) flattened,
    /// then applies defaults. Strict unknown-key rejection for the flattened
    /// section is the `Document` loader's job, since serde cannot combine
    /// `deny_unknown_fields` with `#[serde(flatten)]`.
    fn configure(&self, raw: RawValue) -> AppResult<Box<dyn ConfiguredAdapter>>;

    /// Config-less detection: emit a minimal `[ecosystems.<id>]` fragment if this
    /// ecosystem applies under `project_root`. `None` = not present.
    fn scaffold(&self, project_root: &Path) -> AppResult<Option<EcosystemFragment>>;
}
