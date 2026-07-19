//! [`CommandConfig`] — the parsed, declarative `[ecosystems.command]` schema.

use serde::Deserialize;
use toven_ports::CommonEcosystemConfig;

/// The strict, parsed `[ecosystems.command]` configuration.
///
/// Unlike the tooling-backed adapters, the command ecosystem **declares** its
/// modules (`[[modules]]`) and tasks (`[tasks.*]`, flattened via
/// [`CommonEcosystemConfig`]) rather than probing anything. `deny_unknown_fields`
/// rejects a typo anywhere in the section — the `toml` deserializer honors it
/// across the flattened remainder, so
/// [`CommandProvider::configure`](toven_ports::Provider::configure) surfaces
/// section-level typos itself.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandConfig {
    /// The declared modules this ecosystem orchestrates. Discovery normalizes
    /// these into the federated graph without shelling out.
    #[serde(default)]
    pub modules: Vec<DeclaredModule>,
    /// Optional explicit toolchain probe. When omitted, the adapter derives a
    /// probe from the first declared task's program.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolchain: Option<DeclaredToolchain>,
    /// Engine-common knobs shared by every adapter (`run_strategy`, `release`,
    /// `tasks`), flattened into the same section. For this adapter, `tasks` is
    /// the **only** source of tasks — there are no built-in defaults.
    #[serde(flatten)]
    pub common: CommonEcosystemConfig,
}

/// A single declared module (`[[ecosystems.command.modules]]`).
#[derive(Debug, Clone, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredModule {
    /// Module name, unique within the ecosystem (becomes `command:<name>`).
    pub name: String,
    /// Repo-relative module root.
    pub root: String,
    /// Optional repo-relative manifest path (purely informational metadata).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<String>,
    /// Names of other declared modules this one depends on (intra-ecosystem
    /// edges). Each must reference a declared module.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
}

/// An explicit toolchain probe declaration (`[ecosystems.command.toolchain]`).
#[derive(Debug, Clone, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredToolchain {
    /// Program to execute (`argv[0]`); never a shell string.
    pub program: String,
    /// Program arguments (e.g. `["--version"]`).
    #[serde(default)]
    pub args: Vec<String>,
    /// Optional human-readable label; defaults to `program` for diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::CommandConfig;

    #[test]
    fn empty_section_parses_with_no_modules() {
        let config: CommandConfig = toml::Value::Table(toml::Table::new())
            .try_into()
            .expect("empty section parses");
        assert!(config.modules.is_empty());
        assert!(config.toolchain.is_none());
    }

    #[test]
    fn rejects_unknown_field_across_flatten() {
        let raw: toml::Value = toml::from_str("bogus = 1\n").expect("toml");
        let error = raw
            .try_into::<CommandConfig>()
            .expect_err("unknown field rejected");
        assert!(error.to_string().contains("bogus"), "{error}");
    }

    #[test]
    fn rejects_unknown_module_field() {
        let raw: toml::Value =
            toml::from_str("[[modules]]\nname = \"a\"\nroot = \"a\"\nbogus = 1\n").expect("toml");
        let error = raw
            .try_into::<CommandConfig>()
            .expect_err("unknown module field rejected");
        assert!(error.to_string().contains("bogus"), "{error}");
    }
}
