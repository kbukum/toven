//! [`BaselineSourceConfig`] — the configurable selector for *where* a module's
//! release change-detection baseline is anchored.
//!
//! This is the config-surface counterpart of the engine's `BaselineSource`
//! policy vocabulary. The config names the choice; the release engine resolves
//! it into a concrete baseline source using the module's own tag scheme and the
//! member's umbrella tag scheme. A choice that references the umbrella tag
//! ([`UmbrellaTag`](Self::UmbrellaTag) / [`RegistryUmbrella`](Self::RegistryUmbrella))
//! requires the member to declare an umbrella module, validated at plan time.

use serde::{Deserialize, Serialize};

/// Where a module's release baseline (the change-detection anchor) comes from.
///
/// The variants mirror the two registry models Toven supports: per-module tags
/// *are* the registry (Go → [`OwnTag`](Self::OwnTag)), while a registry plus one
/// umbrella tag describes a Rust workspace ([`RegistryUmbrella`](Self::RegistryUmbrella)).
#[derive(Debug, Clone, Copy, Eq, PartialEq, Deserialize, Serialize)]
#[non_exhaustive]
pub enum BaselineSourceConfig {
    /// Anchor on the module's own latest release tag — the per-module-tag model.
    #[serde(rename = "own-tag")]
    OwnTag,
    /// Anchor on the member's umbrella tag (version and diff ref both from the
    /// umbrella tag). Requires an umbrella module.
    #[serde(rename = "umbrella-tag")]
    UmbrellaTag,
    /// Anchor idempotency on the registry's max published version; the diff ref
    /// comes from the module's own tag.
    #[serde(rename = "registry")]
    Registry,
    /// Anchor idempotency on `max(registry, umbrella-tag)`; the diff ref comes
    /// from the umbrella tag — the composition a Rust workspace with crates.io
    /// history and one umbrella tag needs. Requires an umbrella module.
    #[serde(rename = "registry+umbrella")]
    RegistryUmbrella,
}

impl BaselineSourceConfig {
    /// The stable identifier used in diagnostics and projections.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OwnTag => "own-tag",
            Self::UmbrellaTag => "umbrella-tag",
            Self::Registry => "registry",
            Self::RegistryUmbrella => "registry+umbrella",
        }
    }

    /// Whether resolving this source requires the member to declare an umbrella
    /// module.
    #[must_use]
    pub const fn requires_umbrella(self) -> bool {
        matches!(self, Self::UmbrellaTag | Self::RegistryUmbrella)
    }
}

#[cfg(test)]
mod tests {
    use super::BaselineSourceConfig;

    #[derive(serde::Deserialize)]
    struct Wrap {
        source: BaselineSourceConfig,
    }

    fn parse(value: &str) -> BaselineSourceConfig {
        toml::from_str::<Wrap>(&format!("source = \"{value}\""))
            .expect("parses")
            .source
    }

    #[test]
    fn parses_every_variant() {
        assert_eq!(parse("own-tag"), BaselineSourceConfig::OwnTag);
        assert_eq!(parse("umbrella-tag"), BaselineSourceConfig::UmbrellaTag);
        assert_eq!(parse("registry"), BaselineSourceConfig::Registry);
        assert_eq!(
            parse("registry+umbrella"),
            BaselineSourceConfig::RegistryUmbrella
        );
    }

    #[test]
    fn rejects_unknown_variant() {
        assert!(toml::from_str::<Wrap>("source = \"crates-io\"").is_err());
    }

    #[test]
    fn umbrella_backed_sources_require_an_umbrella() {
        assert!(BaselineSourceConfig::UmbrellaTag.requires_umbrella());
        assert!(BaselineSourceConfig::RegistryUmbrella.requires_umbrella());
        assert!(!BaselineSourceConfig::OwnTag.requires_umbrella());
        assert!(!BaselineSourceConfig::Registry.requires_umbrella());
    }
}
