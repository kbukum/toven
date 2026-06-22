//! Phase 2 — Configure: bake each `[ecosystems.<id>]` raw subtree into a
//! [`ConfiguredAdapter`] via its [`Provider`].
//!
//! The loaded [`Document`] keeps every ecosystem subtree verbatim as a
//! `serde_json`-backed [`RawValue`]; each is handed to the owning provider's
//! [`Provider::configure`] (which parses it under the adapter's own strict
//! schema). Ecosystems with no loaded provider were already classified at Load
//! (canonical-but-unloaded = warn + ignore; unknown = hard error), so they are
//! simply skipped here.

use std::collections::BTreeMap;

use rskit_config::RawValue;
use rskit_errors::{AppError, AppResult};
use toven_model::EcosystemId;
use toven_ports::{ConfiguredAdapter, Provider};

use crate::config::Document;

/// The per-ecosystem configured-adapter set produced by [`configure`].
pub(super) type ConfiguredSet = BTreeMap<EcosystemId, Box<dyn ConfiguredAdapter>>;

/// Configure every loaded ecosystem section of `document`.
///
/// `providers` is the set of ecosystem adapters compiled into this binary. For
/// each `[ecosystems.<id>]` subtree whose ecosystem has a provider, the raw
/// subtree is baked into a [`ConfiguredAdapter`]; subtrees without a provider are
/// skipped (already accepted as canonical-but-unloaded at Load).
///
/// # Errors
/// Propagates a provider's `configure` failure, or a subtree that cannot be
/// converted into the TOML value the provider expects.
pub(super) fn configure(
    document: &Document,
    providers: &[&dyn Provider],
) -> AppResult<ConfiguredSet> {
    let mut by_id: BTreeMap<&EcosystemId, &&dyn Provider> = BTreeMap::new();
    for provider in providers {
        if by_id.insert(provider.ecosystem_id(), provider).is_some() {
            return Err(AppError::new(
                rskit_errors::ErrorCode::Internal,
                format!(
                    "two providers claim ecosystem '{}'",
                    provider.ecosystem_id()
                ),
            ));
        }
    }

    let mut configured = ConfiguredSet::new();
    for (ecosystem, raw) in &document.ecosystems {
        let Some(provider) = by_id.get(ecosystem) else {
            continue;
        };
        let value = to_toml_value(ecosystem, raw)?;
        let adapter = provider.configure(value)?;
        configured.insert(ecosystem.clone(), adapter);
    }
    Ok(configured)
}

/// Convert a `serde_json`-backed [`RawValue`] subtree into the [`toml::Value`]
/// the [`Provider::configure`] contract consumes.
fn to_toml_value(ecosystem: &EcosystemId, raw: &RawValue) -> AppResult<toml::Value> {
    toml::Value::try_from(raw).map_err(|error| {
        AppError::invalid_input(
            format!("ecosystems.{ecosystem}"),
            format!("could not convert configuration subtree: {error}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use toven_model::EcosystemId;
    use toven_ports::Provider;
    use toven_testkit::FakeProvider;

    use super::configure;
    use crate::config::{Document, ProjectConfig, TovenConfig};

    fn eid(id: &str) -> EcosystemId {
        EcosystemId::new(id).unwrap()
    }

    fn empty_document() -> Document {
        Document {
            project: ProjectConfig {
                name: "t".to_string(),
                root: ".".to_string(),
                base_ref: None,
            },
            toven: TovenConfig::default(),
            groups: BTreeMap::new(),
            overlays: Vec::new(),
            ecosystems: BTreeMap::new(),
            members: Vec::new(),
        }
    }

    #[test]
    fn two_providers_claiming_one_ecosystem_is_rejected() {
        let first = FakeProvider::new(eid("rust"));
        let second = FakeProvider::new(eid("rust"));
        let providers: Vec<&dyn Provider> = vec![&first, &second];

        assert!(configure(&empty_document(), &providers).is_err());
    }
}
