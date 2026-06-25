//! The `go` toolchain probe spec.

use toven_ports::ToolchainProbe;

/// The probe the planner runs once per active workspace to compose the opaque
/// toolchain version identity.
///
/// Phase 1 (discovery) only stamps `Workspace.toolchain.tool = "go"`; this spec
/// is executed in phase 2 (after affected-filtering), and its trimmed stdout —
/// e.g. `"go version go1.26.0 darwin/arm64"` — becomes the cache-significant
/// version string.
#[must_use]
pub(crate) fn go_probe() -> ToolchainProbe {
    ToolchainProbe::new("go", "go", vec!["version".to_string()])
}

#[cfg(test)]
mod tests {
    use super::go_probe;

    #[test]
    fn probe_invokes_go_version() {
        let probe = go_probe();
        assert_eq!(probe.program, "go");
        assert_eq!(probe.args, ["version"]);
    }
}
