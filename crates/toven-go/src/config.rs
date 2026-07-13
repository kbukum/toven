//! [`GoConfig`] — the parsed, go-aware `[ecosystems.go]` schema.

use std::fmt;

use serde::de::{self, SeqAccess, Visitor};
use serde::ser::SerializeSeq;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use toven_ports::CommonEcosystemConfig;

/// The `auto` keyword that selects dynamic module discovery.
const AUTO: &str = "auto";

/// Which `go.mod` modules the ecosystem manages.
///
/// Authored either as the `auto` keyword (re-derive the module set every plan
/// from `go.work` or the on-disk layout) or as an explicit, author-frozen list
/// of repo-relative `go.mod` paths. Each entry is one module root.
#[derive(Debug, Clone, Eq, PartialEq, Default)]
pub enum Modules {
    /// Re-derive the module set on every plan: a root `go.work`'s member list
    /// (at any depth), or the root `go.mod` plus every first-level nested
    /// `go.mod` when there is no workspace file. A module added later is picked
    /// up without a config edit.
    #[default]
    Auto,
    /// An explicit, author-frozen list of repo-relative `go.mod` paths.
    Explicit(Vec<String>),
}

impl Serialize for Modules {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Auto => serializer.serialize_str(AUTO),
            Self::Explicit(list) => {
                let mut seq = serializer.serialize_seq(Some(list.len()))?;
                for manifest in list {
                    seq.serialize_element(manifest)?;
                }
                seq.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Modules {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// Accepts the `auto` keyword or a list of `go.mod` paths.
        struct ModulesVisitor;

        impl<'de> Visitor<'de> for ModulesVisitor {
            type Value = Modules;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("\"auto\" or a list of go.mod paths")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                if value == AUTO {
                    Ok(Modules::Auto)
                } else {
                    Err(de::Error::invalid_value(
                        de::Unexpected::Str(value),
                        &"the keyword \"auto\"",
                    ))
                }
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut modules = Vec::new();
                while let Some(module) = seq.next_element::<String>()? {
                    modules.push(module);
                }
                Ok(Modules::Explicit(modules))
            }
        }

        deserializer.deserialize_any(ModulesVisitor)
    }
}

/// The strict, parsed `[ecosystems.go]` configuration.
///
/// `modules` is the adapter-owned knob (`"auto"` or an explicit `go.mod` list,
/// mirroring the Rust adapter's `manifests`); the engine-common knobs
/// (`run_strategy`, `release`, per-task overrides) are flattened in via
/// [`CommonEcosystemConfig`]. `deny_unknown_fields` rejects a typo anywhere in
/// the section — the `toml` deserializer honors it across the flattened
/// remainder, so [`GoProvider::configure`](toven_ports::Provider::configure)
/// surfaces section-level typos itself rather than relying on the document
/// loader.
///
/// A root `go.work` is auto-detected by discovery (it both enumerates the
/// managed modules under `auto` and groups them into one workspace). Selecting
/// the discovery driver is intentionally not exposed yet — the driver is
/// isolated behind [`GoAdapter`](crate::GoAdapter) so a future `driver` knob is
/// a localized addition.
#[derive(Debug, Clone, Eq, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoConfig {
    /// The `go.mod` modules to discover. Defaults to `auto`.
    #[serde(default)]
    pub modules: Modules,
    /// Engine-common knobs shared by every adapter (`run_strategy`, `release`,
    /// `tasks`), flattened into the same section.
    #[serde(flatten)]
    pub common: CommonEcosystemConfig,
}

#[cfg(test)]
mod tests {
    use super::{GoConfig, Modules};

    #[test]
    fn defaults_apply_when_section_is_empty() {
        let config: GoConfig = toml::Value::Table(toml::Table::new())
            .try_into()
            .expect("empty section parses with defaults");
        assert_eq!(config.modules, Modules::Auto);
    }

    #[test]
    fn auto_keyword_parses_to_auto() {
        let config: GoConfig = toml::from_str("modules = \"auto\"\n").expect("auto keyword parses");
        assert_eq!(config.modules, Modules::Auto);
    }

    #[test]
    fn explicit_list_parses_to_explicit() {
        let config: GoConfig =
            toml::from_str("modules = [\"go.mod\", \"auth/go.mod\"]\n").expect("list parses");
        assert_eq!(
            config.modules,
            Modules::Explicit(vec!["go.mod".to_string(), "auth/go.mod".to_string()])
        );
    }

    #[test]
    fn rejects_unknown_keyword() {
        let error =
            toml::from_str::<GoConfig>("modules = \"all\"\n").expect_err("only auto is accepted");
        assert!(error.to_string().contains("auto"), "{error}");
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
