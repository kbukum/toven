//! The `[ecosystems.<id>]` fragment a config-less scaffold emits.

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
#[derive(Debug, Clone, PartialEq)]
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
