//! `[[members]]` — multi-repo umbrella member declarations.

use serde::{Deserialize, Serialize};

/// A reserved `[[members]]` entry: one repo in a multi-repo umbrella.
///
/// `[project]` is the degenerate single-member case; an umbrella adds a
/// `[[members]]` array. Loading each member's own `toven.toml` and composing the
/// federated graph are federation responsibilities; this schema is parsed and
/// structurally validated during config loading.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MemberConfig {
    /// Unique member name within the umbrella.
    pub name: String,
    /// Member repo root relative to the umbrella config file.
    pub root: String,
    /// Optional per-member change baseline ref (else the member's own default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_ref: Option<String>,
}
