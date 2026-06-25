//! [`CommandAdapter`] — the configured escape-hatch adapter the engine drives.

use rskit_errors::AppResult;
use toven_ports::{
    CommonEcosystemConfig, ConfiguredAdapter, DiscoverRequest, DiscoverResponse, ReleaseTarget,
    RunStrategy, Task, TaskKind, ToolchainProbe,
};

use crate::config::CommandConfig;
use crate::discovery;
use crate::tasks;

/// The toolchain program/label for an empty (module-less) ecosystem. This is a
/// stable placeholder, never executed: [`CommandProvider::configure`] rejects a
/// config that declares modules without tasks or a `[toolchain]`, and the engine
/// only probes a workspace that owns an active module — so the degenerate branch
/// is unreachable for any probed workspace.
const DEFAULT_TOOL: &str = "command";

/// The configured command adapter: a baked [`CommandConfig`] plus its resolved
/// (user-declared) task table.
///
/// Constructed by [`CommandProvider::configure`](toven_ports::Provider::configure)
/// and held by the engine as `dyn ConfiguredAdapter`.
#[derive(Debug, Clone)]
pub struct CommandAdapter {
    config: CommandConfig,
    tasks: Vec<Task>,
}

impl CommandAdapter {
    /// Construct an adapter from a baked config and its resolved tasks.
    #[must_use]
    pub const fn new(config: CommandConfig, tasks: Vec<Task>) -> Self {
        Self { config, tasks }
    }
}

impl ConfiguredAdapter for CommandAdapter {
    fn discover(&self, request: &DiscoverRequest) -> AppResult<DiscoverResponse> {
        discovery::discover(&self.config, request)
    }

    fn default_tasks(&self) -> Vec<Task> {
        self.tasks.clone()
    }

    /// Resolve the toolchain probe with no inference beyond what's declared.
    ///
    /// Precedence: an explicit `[toolchain]` block wins; otherwise the first
    /// declared task's program is probed with `--version`. The final
    /// `command --version` placeholder is only reachable for an empty,
    /// module-less ecosystem — which the engine never probes — because
    /// [`CommandProvider::configure`](toven_ports::Provider::configure) rejects
    /// modules declared without any task or `[toolchain]`.
    fn toolchain_probe(&self) -> ToolchainProbe {
        if let Some(toolchain) = &self.config.toolchain {
            let args = if toolchain.args.is_empty() {
                vec!["--version".to_string()]
            } else {
                toolchain.args.clone()
            };
            let label = toolchain
                .label
                .clone()
                .unwrap_or_else(|| toolchain.program.clone());
            return ToolchainProbe::new(label, toolchain.program.clone(), args);
        }

        if let Some(program) = self.tasks.first().and_then(|task| task.argv.first()) {
            return ToolchainProbe::new(
                program.clone(),
                program.clone(),
                vec!["--version".to_string()],
            );
        }

        ToolchainProbe::new(DEFAULT_TOOL, DEFAULT_TOOL, vec!["--version".to_string()])
    }

    fn run_strategy_default(&self, kind: &TaskKind) -> RunStrategy {
        self.config
            .common
            .run_strategy
            .unwrap_or_else(|| tasks::default_run_strategy(kind))
    }

    fn release_target(&self) -> AppResult<Option<Box<dyn ReleaseTarget>>> {
        Ok(None)
    }

    fn common(&self) -> &CommonEcosystemConfig {
        &self.config.common
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use toven_ports::{ConfiguredAdapter, TaskOverride};

    use super::CommandAdapter;
    use crate::config::{CommandConfig, DeclaredToolchain};
    use crate::tasks::resolve_tasks;

    fn task_override(argv: &[&str]) -> TaskOverride {
        TaskOverride {
            argv: Some(argv.iter().map(ToString::to_string).collect()),
            ..TaskOverride::default()
        }
    }

    #[test]
    fn declared_toolchain_wins() {
        let config = CommandConfig {
            toolchain: Some(DeclaredToolchain {
                program: "bazel".to_string(),
                args: vec!["version".to_string()],
                label: Some("bazel-toolchain".to_string()),
            }),
            ..CommandConfig::default()
        };
        let adapter = CommandAdapter::new(config, Vec::new());
        let probe = adapter.toolchain_probe();
        assert_eq!(probe.program, "bazel");
        assert_eq!(probe.args, ["version"]);
    }

    #[test]
    fn first_task_program_is_probed_when_no_toolchain() {
        let mut overrides = BTreeMap::new();
        overrides.insert("build".to_string(), task_override(&["make", "build"]));
        let tasks = resolve_tasks(&overrides).expect("resolves");
        let adapter = CommandAdapter::new(CommandConfig::default(), tasks);
        let probe = adapter.toolchain_probe();
        assert_eq!(probe.program, "make");
        assert_eq!(probe.args, ["--version"]);
    }

    #[test]
    fn degenerate_probe_falls_back_to_command() {
        let adapter = CommandAdapter::new(CommandConfig::default(), Vec::new());
        let probe = adapter.toolchain_probe();
        assert_eq!(probe.program, "command");
    }

    #[test]
    fn never_offers_a_release_target() {
        let adapter = CommandAdapter::new(CommandConfig::default(), Vec::new());
        assert!(adapter.release_target().expect("ok").is_none());
    }
}
