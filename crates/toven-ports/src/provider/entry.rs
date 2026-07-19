//! The stateless registered entry point: id, config baking, and the wizard.

use std::path::Path;

use rskit_config::RawValue;
use rskit_errors::AppResult;
use toven_model::EcosystemId;

use super::{ConfiguredAdapter, EcosystemFragment};
use crate::wizard::{Answers, Detection, Questionnaire};

/// The stateless, id-registered entry point for an ecosystem.
///
/// Held by the loaded-provider registry as `dyn Provider`. Object-safe: it
/// bakes a raw `[ecosystems.<id>]` subtree into a config-bearing
/// [`ConfiguredAdapter`], and (config-less) drives the three-step onboarding
/// wizard — [`detect`](Self::detect) → [`questionnaire`](Self::questionnaire) →
/// [`render`](Self::render).
pub trait Provider {
    /// The ecosystem this provider serves (`[ecosystems.<id>]` key).
    fn ecosystem_id(&self) -> &EcosystemId;

    /// Parse + bake the raw `[ecosystems.<id>]` subtree into a configured
    /// adapter.
    ///
    /// `raw` is the canonical [`RawValue`] subtree retained verbatim by the
    /// loader (format-neutral, regardless of the on-disk source). The adapter
    /// deserializes it into its own typed schema — typically via
    /// [`rskit_config::deserialize_subtree`] — with
    /// [`CommonEcosystemConfig`](crate::config::CommonEcosystemConfig)
    /// flattened, then applies defaults. Strict unknown-key rejection for the
    /// flattened section is the `Document` loader's job, since serde cannot
    /// combine `deny_unknown_fields` with `#[serde(flatten)]`.
    fn configure(&self, raw: RawValue) -> AppResult<Box<dyn ConfiguredAdapter>>;

    /// Config-less detection: probe `project_root` and, if this ecosystem
    /// applies, return a [`Detection`] carrying the adapter's own facts. `None`
    /// = the ecosystem is not present under the root.
    fn detect(&self, project_root: &Path) -> AppResult<Option<Detection>>;

    /// Build the declarative [`Questionnaire`] for a [`Detection`], with the
    /// recommended default preselected from the probe. May be empty (an
    /// ecosystem with nothing to ask still renders a sane default fragment).
    fn questionnaire(&self, detection: &Detection) -> AppResult<Questionnaire>;

    /// Materialize the complete `[ecosystems.<id>]` section — including the
    /// full task table — from a [`Detection`] and the user's [`Answers`].
    fn render(&self, detection: &Detection, answers: &Answers) -> AppResult<EcosystemFragment>;
}
