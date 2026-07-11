//! [`RustConfig`] — the parsed, cargo-aware `[ecosystems.rust]` schema.

use std::fmt;

use serde::de::{self, SeqAccess, Visitor};
use serde::ser::SerializeSeq;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use toven_ports::CommonEcosystemConfig;

/// The `auto` keyword that selects dynamic workspace-root discovery.
const AUTO: &str = "auto";

/// Which Cargo workspace roots the ecosystem manages.
///
/// Authored either as the `auto` keyword (re-discover first-level workspace
/// roots every plan) or as an explicit, author-frozen list of repo-relative
/// manifest paths. Each entry is the root of one `cargo metadata` invocation —
/// a workspace root, never an arbitrary member `Cargo.toml`.
#[derive(Debug, Clone, Eq, PartialEq, Default)]
pub enum Manifests {
    /// Re-discover first-level workspace roots on every plan, minus
    /// [`RustConfig::exclude`]. New workspaces are picked up without a config
    /// edit.
    #[default]
    Auto,
    /// An explicit, author-frozen list of repo-relative workspace-root
    /// manifests.
    Explicit(Vec<String>),
}

impl Serialize for Manifests {
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

impl<'de> Deserialize<'de> for Manifests {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// Accepts the `auto` keyword or a list of manifest paths.
        struct ManifestsVisitor;

        impl<'de> Visitor<'de> for ManifestsVisitor {
            type Value = Manifests;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("\"auto\" or a list of workspace-root manifest paths")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                if value == AUTO {
                    Ok(Manifests::Auto)
                } else {
                    Err(de::Error::invalid_value(
                        de::Unexpected::Str(value),
                        &"the keyword \"auto\"",
                    ))
                }
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut manifests = Vec::new();
                while let Some(manifest) = seq.next_element::<String>()? {
                    manifests.push(manifest);
                }
                Ok(Manifests::Explicit(manifests))
            }
        }

        deserializer.deserialize_any(ManifestsVisitor)
    }
}

/// Crates are publishable by default; `publish = false` opts a project out.
const fn default_publish() -> bool {
    true
}

/// The strict, parsed `[ecosystems.rust]` configuration.
///
/// `manifests`, `exclude`, and `publish` are the adapter-owned knobs; the
/// engine-common knobs (`run_strategy`, `release`, per-task overrides) are
/// flattened in via [`CommonEcosystemConfig`]. `deny_unknown_fields` rejects a
/// typo anywhere in the section — the `toml` deserializer honors it across the
/// flattened remainder, so
/// [`RustProvider::configure`](toven_ports::Provider::configure) surfaces
/// section-level typos itself rather than relying on the document loader.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RustConfig {
    /// The Cargo workspace roots to discover. Defaults to `auto`.
    #[serde(default)]
    pub manifests: Manifests,
    /// Workspace directories (or manifest paths) to skip when `manifests` is
    /// `auto`. Ignored for an explicit manifest list.
    #[serde(default)]
    pub exclude: Vec<String>,
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
            manifests: Manifests::default(),
            exclude: Vec::new(),
            publish: default_publish(),
            common: CommonEcosystemConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Manifests, RustConfig};

    #[test]
    fn defaults_apply_when_section_is_empty() {
        let config: RustConfig = toml::Value::Table(toml::Table::new())
            .try_into()
            .expect("empty section parses with defaults");
        assert_eq!(config.manifests, Manifests::Auto);
        assert!(config.exclude.is_empty());
        assert!(config.publish);
    }

    #[test]
    fn auto_keyword_parses_to_auto() {
        let config: RustConfig =
            toml::from_str("manifests = \"auto\"\n").expect("auto keyword parses");
        assert_eq!(config.manifests, Manifests::Auto);
    }

    #[test]
    fn explicit_list_parses_to_explicit() {
        let config: RustConfig =
            toml::from_str("manifests = [\"core/Cargo.toml\"]\n").expect("explicit list parses");
        assert_eq!(
            config.manifests,
            Manifests::Explicit(vec!["core/Cargo.toml".to_string()])
        );
    }

    #[test]
    fn unknown_manifests_keyword_is_rejected() {
        let error = toml::from_str::<RustConfig>("manifests = \"all\"\n")
            .expect_err("only the auto keyword is accepted");
        assert!(error.to_string().contains("auto"), "{error}");
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
