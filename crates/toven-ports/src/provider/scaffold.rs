//! The `[ecosystems.<id>]` fragment a config-less scaffold emits.

use serde::{Deserialize, Serialize};
use toml::Table;
use toven_model::EcosystemId;

/// A single `[ecosystems.<id>]` config fragment produced by
/// [`Provider::scaffold`](super::Provider::scaffold).
///
/// Generation runs **before** config exists, so the provider self-detects its
/// ecosystem by convention and emits the minimal discovery hints as a raw TOML
/// table; `toven generate` merges every provider's fragment into one
/// polyglot `toven.toml`. Keeping it raw [`Table`] means generate owns rendering
/// and comment preservation, not this port.
///
/// It is (de)serializable so a config-less scaffold can also cross the federated
/// driver transport: `toven generate` probes any out-of-process `toven-<eco>`
/// driver, which returns its fragment over the same framed protocol the PLAN
/// spine uses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EcosystemFragment {
    /// The ecosystem the fragment configures (the `[ecosystems.<id>]` key).
    pub ecosystem: EcosystemId,
    /// The raw table body to splice under `[ecosystems.<id>]`.
    pub table: Table,
}

impl EcosystemFragment {
    /// Construct a fragment for `ecosystem` with the given table body.
    #[must_use]
    pub const fn new(ecosystem: EcosystemId, table: Table) -> Self {
        Self { ecosystem, table }
    }
}

#[cfg(test)]
mod tests {
    use toml::Table;
    use toven_model::EcosystemId;

    use super::EcosystemFragment;

    #[test]
    fn new_carries_ecosystem_and_table() {
        let ecosystem = EcosystemId::new("rust").expect("valid id");
        let fragment = EcosystemFragment::new(ecosystem.clone(), Table::new());
        assert_eq!(fragment.ecosystem, ecosystem);
        assert!(fragment.table.is_empty());
    }
}
