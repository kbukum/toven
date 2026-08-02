//! Tool-audit vocabulary: the presence classification the `doctor` verb reports.
//!
//! A repository's resolved task graph needs a set of tools on `PATH` (each
//! ecosystem adapter names them through its toolchain probes). The engine
//! audits that set and classifies every tool with a [`ToolStatus`]; the CLI
//! projects the result through the same [`Event`](crate::Event) reporter sinks
//! a run uses. The type is pure vocabulary — it neither probes nor prints.

use serde::{Deserialize, Serialize};

/// Whether a probed tool is present (with an optional version) or missing.
///
/// Missing means the probe could not find the program on `PATH` (a spawn
/// `NotFound`); every other probe failure — a hang, a permission error, an
/// output overrun — is a hard error, not a "missing tool", so it never reaches
/// this classification. A present tool that reports no parseable version is
/// [`Present`](ToolStatus::Present) with `version = None`, distinct from
/// [`Missing`](ToolStatus::Missing).
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum ToolStatus {
    /// The tool is installed and on `PATH`; `version` carries its reported
    /// version line when one was parseable.
    Present {
        /// The tool's reported version, when the probe returned one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
    },
    /// The tool could not be found on `PATH`.
    Missing,
}

impl ToolStatus {
    /// Whether this classification is [`Missing`](ToolStatus::Missing).
    #[must_use]
    pub const fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }
}

#[cfg(test)]
mod tests {
    use super::ToolStatus;

    fn round_trip(status: &ToolStatus) {
        let json = serde_json::to_string(status).expect("serialize");
        let back: ToolStatus = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(status, &back);
    }

    #[test]
    fn present_and_missing_round_trip() {
        round_trip(&ToolStatus::Present {
            version: Some("cargo 1.94.0".to_string()),
        });
        round_trip(&ToolStatus::Present { version: None });
        round_trip(&ToolStatus::Missing);
    }

    #[test]
    fn is_missing_only_for_the_missing_variant() {
        assert!(ToolStatus::Missing.is_missing());
        assert!(!ToolStatus::Present { version: None }.is_missing());
    }

    #[test]
    fn a_present_tool_without_a_version_omits_the_field() {
        let json =
            serde_json::to_string(&ToolStatus::Present { version: None }).expect("serialize");
        assert!(!json.contains("version"), "got {json}");
        assert!(json.contains(r#""status":"present""#), "got {json}");
    }
}
