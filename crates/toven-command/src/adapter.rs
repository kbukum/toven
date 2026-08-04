//! [`CommandAdapter`] — the configured escape-hatch adapter the engine drives.

use rskit_errors::AppResult;
use toven_ports::{
    CommonEcosystemConfig, ConfiguredAdapter, DiscoverRequest, DiscoverResponse, ReleaseAdapter,
    RunStrategy, TaskIntent, TaskKind, ToolchainProbe,
};

use crate::config::CommandConfig;
use crate::discovery;
use crate::tasks;

/// The toolchain program/label for an empty (module-less) ecosystem. This is a
/// stable placeholder, never executed: [`CommandProvider::configure`] rejects a
/// config that declares modules without tasks or a `[toolchain]`, and the
/// engine only probes a workspace that owns an active module — so the
/// degenerate branch is unreachable for any probed workspace.
const DEFAULT_TOOL: &str = "command";

/// The configured command adapter: a baked [`CommandConfig`].
///
/// Constructed by
/// [`CommandProvider::configure`](toven_ports::Provider::configure) and held by
/// the engine as `dyn ConfiguredAdapter`. The runnable task table is read from
/// the parsed config (`common().tasks`), not from the adapter.
#[derive(Debug, Clone)]
pub struct CommandAdapter {
    config: CommandConfig,
}

impl CommandAdapter {
    /// Construct an adapter from a baked config.
    #[must_use]
    pub const fn new(config: CommandConfig) -> Self {
        Self { config }
    }
}

impl ConfiguredAdapter for CommandAdapter {
    fn discover(&self, request: &DiscoverRequest) -> AppResult<DiscoverResponse> {
        discovery::discover(&self.config, request)
    }

    /// Resolve the toolchain probe with no inference beyond what's declared.
    ///
    /// Precedence: an explicit `[toolchain]` block wins; otherwise the first
    /// declared task's program is probed with `--version`. The final `command
    /// --version` placeholder is only reachable for an empty, module-less
    /// ecosystem — which the engine never probes — because
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

        if let Some(program) = self
            .config
            .common
            .tasks
            .values()
            .find_map(|entry| entry.argv.first())
        {
            return ToolchainProbe::new(
                program.clone(),
                program.clone(),
                vec!["--version".to_string()],
            );
        }

        ToolchainProbe::new(DEFAULT_TOOL, DEFAULT_TOOL, vec!["--version".to_string()])
    }

    /// Scope the probe to the tool the addressed task actually runs.
    ///
    /// The command ecosystem funnels every declared module into one workspace,
    /// yet each task is an independent tool (`ast-grep` for `structure`,
    /// `mdbook` for `docs-build`). Probing per task — the addressed task's own
    /// `argv[0]`, checked with `--version` — lets each gate surface a typed
    /// missing-tool error for *its* tool without forcing every other command
    /// tool to be installed for an unrelated run. An explicit `[toolchain]`
    /// still wins (it declares one tool for the whole ecosystem); an unknown or
    /// program-less task falls back to the ecosystem default probe.
    fn toolchain_probes_for(&self, intent: &TaskIntent) -> Vec<ToolchainProbe> {
        if self.config.toolchain.is_some() {
            return vec![self.toolchain_probe()];
        }
        if let Some(program) = self
            .config
            .common
            .tasks
            .get(intent.name())
            .and_then(|entry| entry.argv.first())
        {
            return vec![ToolchainProbe::new(
                program.clone(),
                program.clone(),
                vec!["--version".to_string()],
            )];
        }
        vec![self.toolchain_probe()]
    }

    fn run_strategy_default(&self, kind: TaskKind) -> RunStrategy {
        self.config
            .common
            .run_strategy
            .unwrap_or_else(|| tasks::default_run_strategy(kind))
    }

    fn release_target(&self) -> AppResult<Option<Box<dyn ReleaseAdapter>>> {
        Ok(None)
    }

    fn common(&self) -> &CommonEcosystemConfig {
        &self.config.common
    }
}

#[cfg(test)]
mod tests {
    use toven_ports::{CommonEcosystemConfig, ConfiguredAdapter, FanOut, Readiness, TaskEntry};

    use super::CommandAdapter;
    use crate::config::{CommandConfig, DeclaredToolchain};
    use toven_ports::TaskIntent;

    fn task_entry(argv: &[&str]) -> TaskEntry {
        TaskEntry {
            kind: None,
            argv: argv.iter().map(ToString::to_string).collect(),
            selector: Vec::new(),
            fan_out: FanOut::PerModule,
            persistent: false,
            readiness: Readiness::Started,
            readiness_timeout_secs: None,
            cache_args: false,
            cacheable: true,
            fail_if_output: false,
            shared_inputs: Vec::new(),
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
        let adapter = CommandAdapter::new(config);
        let probe = adapter.toolchain_probe();
        assert_eq!(probe.program, "bazel");
        assert_eq!(probe.args, ["version"]);
    }

    #[test]
    fn first_task_program_is_probed_when_no_toolchain() {
        let mut common = CommonEcosystemConfig::default();
        common
            .tasks
            .insert("build".to_string(), task_entry(&["make", "build"]));
        let config = CommandConfig {
            common,
            ..CommandConfig::default()
        };
        let adapter = CommandAdapter::new(config);
        let probe = adapter.toolchain_probe();
        assert_eq!(probe.program, "make");
        assert_eq!(probe.args, ["--version"]);
    }

    #[test]
    fn degenerate_probe_falls_back_to_command() {
        let adapter = CommandAdapter::new(CommandConfig::default());
        let probe = adapter.toolchain_probe();
        assert_eq!(probe.program, "command");
    }

    #[test]
    fn probes_the_addressed_task_tool() {
        // Each command gate is its own tool; the probe follows the addressed
        // task's `argv[0]`, so `structure` checks `ast-grep` and `docs-build`
        // checks `mdbook` — never the other tool.
        let mut common = CommonEcosystemConfig::default();
        common
            .tasks
            .insert("structure".to_string(), task_entry(&["ast-grep", "scan"]));
        common.tasks.insert(
            "docs-build".to_string(),
            task_entry(&["mdbook", "build", "docs"]),
        );
        let adapter = CommandAdapter::new(CommandConfig {
            common,
            ..CommandConfig::default()
        });

        let structure = adapter.toolchain_probes_for(&TaskIntent::resolve("structure"));
        assert_eq!(structure.len(), 1);
        assert_eq!(structure[0].program, "ast-grep");
        assert_eq!(structure[0].args, ["--version"]);

        let docs = adapter.toolchain_probes_for(&TaskIntent::resolve("docs-build"));
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].program, "mdbook");
    }

    #[test]
    fn explicit_toolchain_scopes_every_task_to_the_declared_tool() {
        // A declared `[toolchain]` names one tool for the whole ecosystem, so it
        // supersedes per-task derivation for any addressed task.
        let mut common = CommonEcosystemConfig::default();
        common
            .tasks
            .insert("structure".to_string(), task_entry(&["ast-grep", "scan"]));
        let adapter = CommandAdapter::new(CommandConfig {
            toolchain: Some(DeclaredToolchain {
                program: "bazel".to_string(),
                args: vec!["version".to_string()],
                label: None,
            }),
            common,
            ..CommandConfig::default()
        });
        let probes = adapter.toolchain_probes_for(&TaskIntent::resolve("structure"));
        assert_eq!(probes.len(), 1);
        assert_eq!(probes[0].program, "bazel");
    }

    #[test]
    fn unknown_task_falls_back_to_the_ecosystem_default_probe() {
        let mut common = CommonEcosystemConfig::default();
        common
            .tasks
            .insert("structure".to_string(), task_entry(&["ast-grep", "scan"]));
        let adapter = CommandAdapter::new(CommandConfig {
            common,
            ..CommandConfig::default()
        });
        // No `deploy` task is declared; the probe falls back to the ecosystem
        // default (first declared task's program) rather than panicking.
        let probes = adapter.toolchain_probes_for(&TaskIntent::resolve("deploy"));
        assert_eq!(probes.len(), 1);
        assert_eq!(probes[0].program, "ast-grep");
    }

    #[test]
    fn never_offers_a_release_target() {
        let adapter = CommandAdapter::new(CommandConfig::default());
        assert!(adapter.release_target().expect("ok").is_none());
    }
}
