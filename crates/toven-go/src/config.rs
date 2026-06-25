//! [`GoConfig`] — the parsed, go-aware `[ecosystems.go]` schema.

use serde::Deserialize;
use toven_ports::CommonEcosystemConfig;

/// The default module set when `[ecosystems.go] modules` is omitted: a single
/// root `go.mod`.
fn default_modules() -> Vec<String> {
    vec!["go.mod".to_string()]
}

/// The strict, parsed `[ecosystems.go]` configuration.
///
/// `modules` is the adapter-owned knob (the explicit `go.mod` files to
/// discover, mirroring the Rust adapter's `manifests`); the engine-common knobs
/// (`run_strategy`, `release`, per-task overrides) are flattened in via
/// [`CommonEcosystemConfig`]. `deny_unknown_fields` rejects a typo anywhere in
/// the section — the `toml` deserializer honors it across the flattened
/// remainder, so [`GoProvider::configure`](toven_ports::Provider::configure)
/// surfaces section-level typos itself rather than relying on the document
/// loader.
///
/// A root `go.work` is auto-detected by discovery (it groups the listed modules
/// into one workspace); it is not configured here. Selecting the discovery
/// driver is intentionally not exposed yet — the driver is isolated inside the
/// [`discovery`](crate::GoAdapter) module so a future `driver` knob is a
/// localized addition.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoConfig {
    /// `go.mod` files to discover, repo-relative. Each is one module root; a
    /// root `go.work` (auto-detected) groups its members into one workspace.
    #[serde(default = "default_modules")]
    pub modules: Vec<String>,
    /// Engine-common knobs shared by every adapter (`run_strategy`, `release`,
    /// `tasks`), flattened into the same section.
    #[serde(flatten)]
    pub common: CommonEcosystemConfig,
}

impl Default for GoConfig {
    fn default() -> Self {
        Self {
            modules: default_modules(),
            common: CommonEcosystemConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::GoConfig;

    #[test]
    fn defaults_apply_when_section_is_empty() {
        let config: GoConfig = toml::Value::Table(toml::Table::new())
            .try_into()
            .expect("empty section parses with defaults");
        assert_eq!(config.modules, ["go.mod"]);
    }

    #[test]
    fn rejects_unknown_field_across_flatten() {
        let raw: toml::Value = toml::from_str("bogus = 1\n").expect("toml");
        let error = raw
            .try_into::<GoConfig>()
            .expect_err("unknown field rejected");
        assert!(error.to_string().contains("bogus"), "{error}");
    }
}
