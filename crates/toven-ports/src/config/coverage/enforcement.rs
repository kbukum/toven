//! Enforcement vocabulary: whether a below-threshold verdict fails the gate.

use serde::{Deserialize, Serialize};

/// How a below-threshold coverage verdict is enforced for a scope.
///
/// `Block` fails the gate closed (a non-zero exit); `Advisory` measures and
/// reports the shortfall but never fails — the typed equivalent of codecov's
/// `informational: true`. `Block` is the default.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Enforcement {
    /// Fail the gate closed when a measured dimension is below its threshold.
    #[default]
    Block,
    /// Measure and report the shortfall, but never fail the gate.
    Advisory,
}

impl Enforcement {
    /// Canonical config/report name for the enforcement mode.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Advisory => "advisory",
        }
    }

    /// Whether this is the default [`Block`](Enforcement::Block) mode.
    #[must_use]
    pub const fn is_block(self) -> bool {
        matches!(self, Self::Block)
    }
}
