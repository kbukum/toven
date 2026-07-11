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
    /// How live per-unit output is rendered on an interactive terminal.
    #[serde(default, skip_serializing_if = "ViewMode::is_default")]
    pub view: ViewMode,
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

/// How live per-unit output is rendered while a run executes on a terminal.
///
/// Only shapes the interactive rendering of live child output; it never changes
/// the typed [`Event`](toven_model::Event) stream or the machine-readable JSON
/// projection. When output is redirected, piped, or non-interactive the renderer
/// always falls back to the linear `stream` shape regardless of this setting.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum ViewMode {
    /// Pick the richest shape the environment supports: `panes` inside a
    /// multiplexer, `tiles` on a plain terminal, `stream` otherwise (the default).
    #[default]
    Auto,
    /// One live, fixed-height tile per in-flight unit in a single terminal.
    Tiles,
    /// One multiplexer pane per unit (opt-in; requires a supported multiplexer).
    Panes,
    /// A single linear stream with no live area: normal-unit output is buffered
    /// into one labeled block when concurrency could interleave it, and streamed
    /// inline for live-safe runs (the log-friendly fallback).
    Stream,
}

impl ViewMode {
    /// Whether this is the default (`Auto`), so it can be skipped on serialize.
    #[must_use]
    pub const fn is_default(&self) -> bool {
        matches!(self, Self::Auto)
    }
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
