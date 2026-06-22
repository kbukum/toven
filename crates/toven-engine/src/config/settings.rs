//! `[toven]` — engine settings (reporting, concurrency, cache, include, drivers).

use std::collections::BTreeMap;

use rskit_config::RawValue;
use serde::{Deserialize, Serialize};

/// The reserved `[toven]` section: engine-level settings.
///
/// Deliberately small. There are no global wave-ordering knobs here — ordering
/// is per-kind/per-ecosystem (`[ecosystems.*]`) so a global override can never
/// silently reshape execution everywhere.
#[derive(Debug, Clone, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TovenConfig {
    /// How runs are reported to the user.
    #[serde(default)]
    pub report: ReportFormat,
    /// Global concurrency ceiling; `None` lets the engine pick a default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_parallel: Option<usize>,
    /// Task-cache settings.
    #[serde(default, skip_serializing_if = "CacheConfig::is_default")]
    pub cache: CacheConfig,
    /// Optional include files merged beneath the canonical document as defaults.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<String>,
    /// Out-of-process driver settings, kept verbatim for the federation step.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub drivers: BTreeMap<String, RawValue>,
}

/// How a run is reported to the user.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum ReportFormat {
    /// Human-readable terminal output (the default).
    #[default]
    Human,
    /// Machine-readable JSON lines.
    Json,
}

/// The `[toven.cache]` sub-section: task-cache root override.
#[derive(Debug, Clone, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CacheConfig {
    /// Cache-root override (workspace-relative); resolution precedence and the
    /// `TOVEN_CACHE_DIR` env override are applied by the engine cache layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
}

impl CacheConfig {
    /// Whether this config is entirely default (so it can be skipped on serialize).
    #[must_use]
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}
