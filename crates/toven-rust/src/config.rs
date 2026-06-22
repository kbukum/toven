//! [`RustConfig`] — the parsed, cargo-aware `[ecosystems.rust]` schema.

use serde::Deserialize;
use toven_ports::CommonEcosystemConfig;

/// The default manifest set when `[ecosystems.rust] manifests` is omitted: a
/// single root `Cargo.toml`.
fn default_manifests() -> Vec<String> {
    vec!["Cargo.toml".to_string()]
}

/// Crates are publishable by default; `publish = false` opts a project out.
const fn default_publish() -> bool {
    true
}

/// The strict, parsed `[ecosystems.rust]` configuration.
///
/// `manifests` and `publish` are the adapter-owned knobs; the engine-common
/// knobs (`run_strategy`, `release`, per-task overrides) are flattened in via
/// [`CommonEcosystemConfig`]. `deny_unknown_fields` rejects a typo anywhere in
/// the section — the `toml` deserializer honors it across the flattened
/// remainder, so [`RustProvider::configure`](toven_ports::Provider::configure)
/// surfaces section-level typos itself rather than relying on the document
/// loader.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RustConfig {
    /// Cargo workspace/crate manifests to discover, repo-relative. Each manifest
    /// is the root of one `cargo metadata` invocation.
    #[serde(default = "default_manifests")]
    pub manifests: Vec<String>,
    /// Whether modules in this ecosystem are publishable to crates.io. `false`
    /// makes [`release_target`](crate::RustAdapter) hand back `None`.
    #[serde(default = "default_publish")]
    pub publish: bool,
    /// Engine-common knobs shared by every adapter (`run_strategy`, `release`,
    /// `tasks`), flattened into the same section.
    #[serde(flatten)]
    pub common: CommonEcosystemConfig,
}

impl Default for RustConfig {
    fn default() -> Self {
        Self {
            manifests: default_manifests(),
            publish: default_publish(),
            common: CommonEcosystemConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RustConfig;

    #[test]
    fn defaults_apply_when_section_is_empty() {
        let config: RustConfig = toml::Value::Table(toml::Table::new())
            .try_into()
            .expect("empty section parses with defaults");
        assert_eq!(config.manifests, ["Cargo.toml"]);
        assert!(config.publish);
    }

    #[test]
    fn rejects_unknown_field_across_flatten() {
        let raw: toml::Value = toml::from_str("bogus = 1\n").expect("toml");
        let error = raw
            .try_into::<RustConfig>()
            .expect_err("unknown field rejected");
        assert!(error.to_string().contains("bogus"), "{error}");
    }
}
