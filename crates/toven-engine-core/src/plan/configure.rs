//! Configure: bake each `[ecosystems.<id>]` raw subtree into a
//! [`ConfiguredAdapter`] via its [`Provider`].
//!
//! The loaded [`Document`] keeps every ecosystem subtree verbatim as a
//! `serde_json`-backed [`RawValue`](rskit_config::RawValue); each is handed to
//! the owning provider's
//! [`Provider::configure`] (which parses it under the adapter's own strict
//! schema). Ecosystems with no loaded provider were already classified at Load
//! (canonical-but-unloaded = warn + ignore; unknown = hard error), so they are
//! simply skipped here.

use std::collections::BTreeMap;

use rskit_errors::{AppError, AppResult};
use toven_model::{EcosystemId, MemberId};
use toven_ports::{ConfiguredAdapter, Provider, TaskKind};

use crate::config::Document;

/// The per-ecosystem configured-adapter set produced by [`configure`].
#[allow(clippy::redundant_pub_crate)]
pub type ConfiguredSet = BTreeMap<EcosystemId, Box<dyn ConfiguredAdapter>>;

/// The configured adapters of a whole federation, partitioned by member.
///
/// Each cross-repo member carries its own authoritative `[ecosystems.*]`
/// config, so two members exposing the same ecosystem (`rust`) hold *distinct*
/// configured adapters. Keying by `Option<MemberId>` keeps them apart; the
/// degenerate single-repo case is one entry under the `None` member, so a
/// lookup with a `None` member resolves exactly like the old single
/// [`ConfiguredSet`].
#[derive(Default)]
#[allow(clippy::redundant_pub_crate)]
pub struct MemberAdapters {
    root: Option<ConfiguredSet>,
    by_member: BTreeMap<MemberId, ConfiguredSet>,
}

impl MemberAdapters {
    /// Install one member's configured-adapter set.
    pub(crate) fn insert(&mut self, member: Option<MemberId>, adapters: ConfiguredSet) {
        if let Some(member) = member {
            self.by_member.insert(member, adapters);
        } else {
            self.root = Some(adapters);
        }
    }

    /// Look up the configured adapter that owns `ecosystem` within `member`.
    pub fn get(
        &self,
        member: Option<&MemberId>,
        ecosystem: &EcosystemId,
    ) -> Option<&dyn ConfiguredAdapter> {
        self.set_for(member)
            .and_then(|set| set.get(ecosystem))
            .map(AsRef::as_ref)
    }

    /// Borrow one member's whole configured-adapter set.
    pub(crate) fn set_for(&self, member: Option<&MemberId>) -> Option<&ConfiguredSet> {
        member.map_or(self.root.as_ref(), |member| self.by_member.get(member))
    }

    /// Iterate every `(member, ecosystem, adapter)` triple across the
    /// federation.
    pub fn iter(
        &self,
    ) -> impl Iterator<Item = (Option<&MemberId>, &EcosystemId, &dyn ConfiguredAdapter)> {
        self.root
            .iter()
            .flat_map(|set| {
                set.iter()
                    .map(|(ecosystem, adapter)| (None, ecosystem, adapter.as_ref()))
            })
            .chain(self.by_member.iter().flat_map(|(member, set)| {
                set.iter()
                    .map(move |(ecosystem, adapter)| (Some(member), ecosystem, adapter.as_ref()))
            }))
    }
}

/// Configure every loaded ecosystem section of `document`.
///
/// `providers` is the set of ecosystem adapters compiled into this binary. For
/// each `[ecosystems.<id>]` subtree whose ecosystem has a provider, the raw
/// subtree is baked into a [`ConfiguredAdapter`]; subtrees without a provider
/// are skipped (already accepted as canonical-but-unloaded at Load).
///
/// # Errors
/// Propagates a provider's `configure` failure, or a subtree that cannot be
/// converted into the TOML value the provider expects.
#[allow(clippy::redundant_pub_crate)]
pub fn configure(document: &Document, providers: &[&dyn Provider]) -> AppResult<ConfiguredSet> {
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
        let adapter = provider.configure(raw.clone())?;
        configured.insert(ecosystem.clone(), adapter);
    }
    Ok(configured)
}

/// The user-addressable task names declared across every configured ecosystem
/// that can collide with a reserved verb.
///
/// A name is reported when it can be typed as `toven <name>` *and* is not
/// itself a recognized-kind canonical name (`build`/`test`/…). Recognized-kind
/// names map by design to their verb, so only non-recognized names (a renamed
/// or custom task) can genuinely shadow a reserved verb. The CLI uses this for
/// load-time collision warnings without re-deriving the configuration itself.
///
/// # Errors
/// Propagates `configure` failures (provider conflicts, subtree conversion, or
/// a provider's `configure` rejection).
pub fn addressable_task_names(
    document: &Document,
    providers: &[&dyn Provider],
) -> AppResult<Vec<String>> {
    let configured = configure(document, providers)?;
    let mut names = Vec::new();
    for (ecosystem, adapter) in &configured {
        for (key, entry) in &adapter.common().tasks {
            let task = entry.materialize(ecosystem.as_str(), key)?;
            if TaskKind::from_name(&task.name).is_none() {
                names.push(task.name);
            }
        }
    }
    Ok(names)
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
            modules: std::collections::BTreeMap::new(),
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
