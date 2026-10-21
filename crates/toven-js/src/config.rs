//! [`JsConfig`] — the parsed, JavaScript-aware `[ecosystems.js]` schema.

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use toven_ports::CommonEcosystemConfig;

use crate::package_manager::Tool;

/// The `auto` keyword that selects dynamic root discovery.
const AUTO: &str = "auto";

/// Which JavaScript roots the ecosystem manages.
///
/// A root is a directory with its own `package.json` and its own install: a
/// lockfile, a workspace definition, or a `packageManager` pin. Authored either
/// as the `auto` keyword or as an explicit, author-frozen list of repo-relative
/// root `package.json` paths.
#[derive(Debug, Clone, Eq, PartialEq, Default)]
pub enum Roots {
    /// Re-derive the root set on every plan: the repository-root
    /// `package.json`, plus every nested `package.json` that is not a workspace
    /// member and carries a root marker. A root added later is picked up
    /// without a config edit.
    #[default]
    Auto,
    /// An explicit, author-frozen list of repo-relative root `package.json`
    /// paths.
    Explicit(Vec<String>),
}

impl<'de> Deserialize<'de> for Roots {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// Accepts the `auto` keyword or a list of `package.json` paths.
        struct RootsVisitor;

        impl<'de> Visitor<'de> for RootsVisitor {
            type Value = Roots;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("\"auto\" or a list of root package.json paths")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                if value == AUTO {
                    Ok(Roots::Auto)
                } else {
                    Err(de::Error::invalid_value(
                        de::Unexpected::Str(value),
                        &"the keyword \"auto\"",
                    ))
                }
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut roots = Vec::new();
                while let Some(root) = seq.next_element::<String>()? {
                    roots.push(root);
                }
                Ok(Roots::Explicit(roots))
            }
        }

        deserializer.deserialize_any(RootsVisitor)
    }
}

impl Serialize for Roots {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Auto => serializer.serialize_str(AUTO),
            Self::Explicit(roots) => roots.serialize(serializer),
        }
    }
}

/// The strict, parsed `[ecosystems.js]` configuration.
///
/// `roots`, `exclude`, `package_manager`, and `pack` are the adapter-owned
/// knobs; the
/// engine-common knobs (`run_strategy`, `release`, per-task entries) are
/// flattened in via [`CommonEcosystemConfig`]. `deny_unknown_fields` rejects a
/// typo anywhere in the section.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsConfig {
    /// The JavaScript roots to discover. Defaults to `auto`.
    #[serde(default)]
    pub roots: Roots,
    /// Repo-relative directories the discovery walk skips entirely: no root
    /// and no workspace member is found under them.
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Per-root package manager overrides, keyed by the root directory (`.`
    /// for the repository root). An override wins over every detected signal;
    /// use it only for repositories whose signals cannot be fixed.
    #[serde(default)]
    pub package_manager: BTreeMap<String, Tool>,
    /// Per-root release pack commands, keyed by the root directory (`.` for
    /// the repository root). A root listed here packs each package with this
    /// argv, run from the repository root, instead of its package manager's
    /// `pack`. Arguments may use `{module.root}` and must pass
    /// `{destination}`, where the command writes exactly one tarball.
    #[serde(default)]
    pub pack: BTreeMap<String, Vec<String>>,
    /// Engine-common knobs shared by every adapter, flattened into the same
    /// section.
    #[serde(flatten)]
    pub common: CommonEcosystemConfig,
}

#[cfg(test)]
mod tests {
    use super::{JsConfig, Roots};
    use crate::package_manager::Tool;

    #[test]
    fn defaults_apply_when_section_is_empty() {
        let config: JsConfig = toml::from_str("").expect("empty section parses");
        assert_eq!(config.roots, Roots::Auto);
        assert!(config.exclude.is_empty());
        assert!(config.package_manager.is_empty());
    }

    #[test]
    fn explicit_roots_and_overrides_parse() {
        let config: JsConfig = toml::from_str(
            "roots = [\"web/package.json\"]\nexclude = [\"examples\"]\n\
             package_manager = { web = \"bun\" }\n",
        )
        .expect("parses");
        assert_eq!(
            config.roots,
            Roots::Explicit(vec!["web/package.json".to_string()])
        );
        assert_eq!(config.exclude, ["examples"]);
        assert_eq!(config.package_manager.get("web"), Some(&Tool::Bun));
    }

    #[test]
    fn roots_serialize_as_authored() {
        #[derive(serde::Serialize)]
        struct Doc {
            roots: Roots,
        }
        let auto = toml::to_string(&Doc { roots: Roots::Auto }).unwrap();
        assert_eq!(auto, "roots = \"auto\"\n");
        let explicit = Roots::Explicit(vec!["web/package.json".to_string()]);
        let text = toml::to_string(&Doc {
            roots: explicit.clone(),
        })
        .unwrap();
        assert_eq!(toml::from_str::<JsConfig>(&text).unwrap().roots, explicit);
    }

    #[test]
    fn rejects_unknown_keyword_and_unknown_manager() {
        let error = toml::from_str::<JsConfig>("roots = \"all\"\n").expect_err("only auto");
        assert!(error.to_string().contains("auto"), "{error}");
        let error = toml::from_str::<JsConfig>("package_manager = { web = \"deno\" }\n")
            .expect_err("unknown manager");
        assert!(error.to_string().contains("deno"), "{error}");
    }

    #[test]
    fn rejects_unknown_field_across_flatten() {
        let raw: toml::Value = toml::from_str("bogus = 1\n").expect("toml");
        let error = raw.try_into::<JsConfig>().expect_err("unknown field");
        assert!(error.to_string().contains("bogus"), "{error}");
    }
}
