//! The JavaScript toolchain probes.
//!
//! Discovery stamps each workspace with its package manager (`npm`, `pnpm`,
//! `yarn`, or `bun`) and leaves the version unresolved. After affected
//! filtering, the planner runs the probes for that tag and joins their output
//! into the cache-significant toolchain identity: the Node runtime plus the
//! package manager, or Bun alone, because Bun is its own runtime.

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_model::ToolchainTag;
use toven_ports::ToolchainProbe;

use crate::package_manager::Tool;

/// The Node.js runtime program.
const NODE: &str = "node";

/// The `--version` flag every probed program accepts.
const VERSION: &str = "--version";

/// A `<program> --version` probe.
fn version_probe(program: &str) -> ToolchainProbe {
    ToolchainProbe::new(program, program, vec![VERSION.to_string()])
}

/// The ecosystem-wide probe: the Node.js runtime.
#[must_use]
pub(crate) fn node_probe() -> ToolchainProbe {
    version_probe(NODE)
}

/// The probes for one workspace's toolchain tag.
///
/// # Errors
/// A tag this adapter never emits, which means the plan did not come from
/// this adapter's discovery.
pub(crate) fn probes_for(toolchain: &ToolchainTag) -> AppResult<Vec<ToolchainProbe>> {
    if toolchain.tool == NODE {
        return Ok(vec![node_probe()]);
    }
    match Tool::from_tag(&toolchain.tool) {
        Some(Tool::Bun) => Ok(vec![version_probe(Tool::Bun.name())]),
        Some(tool) => Ok(vec![node_probe(), version_probe(tool.name())]),
        None => Err(AppError::new(
            ErrorCode::Internal,
            format!(
                "the JavaScript adapter has no toolchain probe for tool '{}'",
                toolchain.tool
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use toven_model::ToolchainTag;

    use super::{node_probe, probes_for};

    fn programs(tool: &str) -> Vec<String> {
        probes_for(&ToolchainTag::new(tool))
            .expect("probes")
            .into_iter()
            .map(|probe| format!("{} {}", probe.program, probe.args.join(" ")))
            .collect()
    }

    #[test]
    fn node_is_the_ecosystem_probe() {
        let probe = node_probe();
        assert_eq!(probe.program, "node");
        assert_eq!(probe.args, ["--version"]);
    }

    #[test]
    fn package_managers_probe_node_and_themselves() {
        assert_eq!(programs("npm"), ["node --version", "npm --version"]);
        assert_eq!(programs("pnpm"), ["node --version", "pnpm --version"]);
        assert_eq!(programs("yarn"), ["node --version", "yarn --version"]);
        assert_eq!(programs("node"), ["node --version"]);
    }

    #[test]
    fn bun_is_its_own_runtime() {
        assert_eq!(programs("bun"), ["bun --version"]);
    }

    #[test]
    fn unknown_tags_are_rejected() {
        let error = probes_for(&ToolchainTag::new("deno")).expect_err("unknown tag");
        assert!(error.to_string().contains("deno"), "{error}");
    }
}
