//! Toolchain probe specification.

use serde::{Deserialize, Serialize};

/// A command the adapter runs once per active workspace to compose the opaque
/// toolchain version identity.
///
/// The engine executes this in `workspace.root` (with a timeout + captured,
/// capped output) and folds the trimmed stdout into
/// [`ToolchainTag::version`](toven_model::ToolchainTag). The adapter owns what
/// counts as cache-significant; this type only carries the probe spec.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
pub struct ToolchainProbe {
    /// Human-readable label for diagnostics (e.g. `"cargo"`).
    pub label: String,
    /// Program to execute (`argv[0]`); never a shell string.
    pub program: String,
    /// Program arguments (e.g. `["--version"]`).
    #[serde(default)]
    pub args: Vec<String>,
}

impl ToolchainProbe {
    /// Construct a probe spec.
    #[must_use]
    pub fn new(label: impl Into<String>, program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            label: label.into(),
            program: program.into(),
            args,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ToolchainProbe;

    #[test]
    fn new_captures_label_program_and_args() {
        let probe = ToolchainProbe::new("cargo", "cargo", vec!["--version".into()]);
        assert_eq!(probe.label, "cargo");
        assert_eq!(probe.program, "cargo");
        assert_eq!(probe.args, vec!["--version".to_string()]);
    }

    #[test]
    fn round_trips_through_toml() {
        let probe = ToolchainProbe::new("cargo", "cargo", vec!["--version".into()]);
        let json = toml::to_string(&probe).expect("serialize");
        let back: ToolchainProbe = toml::from_str(&json).expect("deserialize");
        assert_eq!(probe, back);
    }
}
