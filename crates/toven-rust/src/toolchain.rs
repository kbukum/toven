//! The cargo toolchain probe spec.

use toven_ports::ToolchainProbe;

/// The probe the planner runs once per active workspace to compose the opaque
/// toolchain version identity.
///
/// Discovery only stamps `Workspace.toolchain.tool = "cargo"`; this
/// spec is executed after affected-filtering, and its trimmed
/// stdout — e.g. `"cargo 1.94.0 (… 2026-03-24)"` — becomes the cache-significant
/// version string. The companion `rustc --version` identity is composed by the
/// engine's toolchain resolution on top of this spec; the port carries a single
/// probe.
#[must_use]
pub(crate) fn cargo_probe() -> ToolchainProbe {
    ToolchainProbe::new("cargo", "cargo", vec!["--version".to_string()])
}

#[cfg(test)]
mod tests {
    use super::cargo_probe;

    #[test]
    fn probe_invokes_cargo_version() {
        let probe = cargo_probe();
        assert_eq!(probe.program, "cargo");
        assert_eq!(probe.args, ["--version"]);
    }
}
