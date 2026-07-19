//! `CommonEcosystemConfig` — the engine-common knobs every adapter flattens in.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{CoverageConfig, ReleaseConfig, RunStrategy, TaskEntry};

/// The engine-common `[ecosystems.<id>]` knobs shared by every adapter config.
///
/// Each adapter's typed config embeds this with `#[serde(flatten)]`, so a single
/// deserialize covers the adapter's own fields plus these common ones and lets
/// the adapter set ecosystem-aware defaults for both. Note serde cannot combine
/// `deny_unknown_fields` with `#[serde(flatten)]`: unknown-key (typo) rejection
/// for flattened ecosystem sections is therefore enforced by the strict
/// `Document` loader, not by this parse. The engine reads the knobs back via
/// [`ConfiguredAdapter::common`](crate::provider::ConfiguredAdapter::common).
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct CommonEcosystemConfig {
    /// Ecosystem-level wave-ordering override (else the per-kind adapter default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_strategy: Option<RunStrategy>,
    /// Release sub-config (`release.strategy`, `release.registry`).
    #[serde(default, skip_serializing_if = "ReleaseConfig::is_default")]
    pub release: ReleaseConfig,
    /// Coverage gating sub-config (`coverage.line`, `coverage.enforcement`).
    #[serde(default, skip_serializing_if = "CoverageConfig::is_default")]
    pub coverage: CoverageConfig,
    /// The complete ecosystem task table: one authored [`TaskEntry`] per task
    /// name, the authoritative source of runnable tasks (`toven init` writes it).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tasks: BTreeMap<String, TaskEntry>,
}

#[cfg(test)]
mod tests {
    use super::{CommonEcosystemConfig, RunStrategy};
    use serde::{Deserialize, Serialize};

    /// A stand-in adapter config that flattens the common knobs alongside its own
    /// ecosystem-specific fields. (Section-level `deny_unknown_fields` strictness
    /// is the strict `Document` loader's job: serde does not enforce
    /// `deny_unknown_fields` together with `#[serde(flatten)]`.)
    #[derive(Debug, PartialEq, Deserialize, Serialize)]
    struct FakeAdapterConfig {
        manifests: Vec<String>,
        #[serde(flatten)]
        common: CommonEcosystemConfig,
    }

    #[test]
    fn flatten_round_trips_through_toml() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/config/ecosystems/rust/adapter.toml"
        ));

        let parsed: FakeAdapterConfig = toml::from_str(source).expect("parses");
        assert_eq!(parsed.manifests, ["Cargo.toml", "contrib/Cargo.toml"]);
        assert_eq!(parsed.common.run_strategy, Some(RunStrategy::LeafToTop));
        assert_eq!(parsed.common.release.registry.as_deref(), Some("crates-io"));

        let test = parsed.common.tasks.get("test").expect("test entry present");
        assert_eq!(
            test.argv,
            ["cargo", "nextest", "run", "{module.selector}", "{args}"]
        );
        assert!(test.cache_args);
        assert_eq!(test.shared_inputs, ["rust-toolchain.toml"]);

        let reserialized = toml::to_string(&parsed).expect("serializes");
        let reparsed: FakeAdapterConfig = toml::from_str(&reserialized).expect("re-parses");
        assert_eq!(parsed, reparsed);
    }

    #[test]
    fn task_entry_section_rejects_unknown_field() {
        // A leaf (non-flattened) section enforces its own strictness.
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/config/invalid/unknown-task-field.toml"
        ));
        let error = toml::from_str::<CommonEcosystemConfig>(source)
            .expect_err("unknown TaskEntry field must be rejected");
        assert!(error.to_string().contains("bogus"), "{error}");
    }
}
