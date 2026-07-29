//! `[toven]` — engine settings (reporting, concurrency, cache, include,
//! drivers).

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
    /// Optional include files merged beneath the canonical document as
    /// defaults.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<String>,
    /// Out-of-process driver settings, kept verbatim for the federation step.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub drivers: BTreeMap<String, RawValue>,
    /// Git transport auth for release push/fetch.
    #[serde(default, skip_serializing_if = "GitConfig::is_default")]
    pub git: GitConfig,
}

/// The `[toven.git]` sub-section: how the embedded git backend authenticates
/// network operations (push/fetch).
///
/// These apply whenever the engine performs a git network operation — primarily
/// the release push, but also the fetches behind planning and change selection,
/// which reuse the same repository handle.
///
/// Forge-agnostic by design: Toven owns the *policy* of which environment
/// variables may carry a push token, while the git layer owns the *mechanism*
/// (read the first present var, use it as the HTTPS basic-auth password). The
/// default lists the GitHub Actions token names, but any forge's token variable
/// can be substituted or added.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GitConfig {
    /// Ordered environment variable names consulted for a push/fetch token; the
    /// first present, non-empty value is used as the HTTPS token-as-password.
    /// When none are set (for example local development) the backend falls back
    /// to its ambient transport default.
    #[serde(default = "default_push_token_env")]
    pub push_token_env: Vec<String>,
}

fn default_push_token_env() -> Vec<String> {
    vec!["GITHUB_TOKEN".to_string(), "GH_TOKEN".to_string()]
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            push_token_env: default_push_token_env(),
        }
    }
}

impl GitConfig {
    /// Whether this config is entirely default (so it can be skipped on
    /// serialize).
    #[must_use]
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::{GitConfig, TovenConfig};

    #[test]
    fn git_config_defaults_to_github_token_names() {
        assert_eq!(
            GitConfig::default().push_token_env,
            vec!["GITHUB_TOKEN".to_string(), "GH_TOKEN".to_string()]
        );
        assert!(GitConfig::default().is_default());
        assert!(TovenConfig::default().git.is_default());
    }

    #[test]
    fn git_section_parses_custom_forge_agnostic_token_names() {
        let toven: TovenConfig =
            toml::from_str("[git]\npush_token_env = [\"GITLAB_TOKEN\", \"CI_JOB_TOKEN\"]\n")
                .expect("parse [toven.git]");
        assert_eq!(
            toven.git.push_token_env,
            vec!["GITLAB_TOKEN".to_string(), "CI_JOB_TOKEN".to_string()]
        );
        assert!(!toven.git.is_default());
    }

    #[test]
    fn git_section_rejects_unknown_keys() {
        let err = toml::from_str::<TovenConfig>("[git]\ntoken = \"secret\"\n")
            .expect_err("unknown key rejected");
        assert!(err.to_string().contains("token"), "{err}");
    }

    #[test]
    fn default_git_config_is_skipped_on_serialize() {
        let serialized = toml::to_string(&TovenConfig::default()).expect("serialize");
        assert!(
            !serialized.contains("push_token_env"),
            "default git config must not serialize: {serialized}"
        );
    }
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
/// projection. When output is redirected, piped, or non-interactive the
/// renderer always falls back to the linear `stream` shape regardless of this
/// setting.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum ViewMode {
    /// Pick the richest shape the environment supports: `panes` inside a
    /// multiplexer, `tiles` on a plain terminal, `stream` otherwise (the
    /// default).
    #[default]
    Auto,
    /// One live, content-sized tile per in-flight unit in a single terminal.
    Tiles,
    /// One multiplexer pane per unit (opt-in; requires a supported
    /// multiplexer).
    Panes,
    /// A single linear stream with no live area: normal-unit output is buffered
    /// into one labeled block when concurrency could interleave it, and
    /// streamed inline for live-safe runs (the log-friendly fallback).
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
    /// Whether this config is entirely default (so it can be skipped on
    /// serialize).
    #[must_use]
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}
