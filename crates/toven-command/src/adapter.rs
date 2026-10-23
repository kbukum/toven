//! [`CommandAdapter`] — the configured escape-hatch adapter the engine drives.

use rskit_errors::AppResult;
use toven_model::ToolchainTag;
use toven_ports::{
    CommonEcosystemConfig, ConfiguredAdapter, DiscoverRequest, DiscoverResponse, ReleaseAdapter,
    RunStrategy, TaskIntent, TaskKind, ToolchainProbe,
};

use crate::config::CommandConfig;
use crate::discovery;
use crate::tasks;

/// The toolchain label for an ecosystem with no `PATH` tool to probe (no
/// `[toolchain]`, and every task program is a path or there are no tasks). It
/// is a stable placeholder, never executed: per-task probing returns no probe
/// in that case.
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

    /// The probe for the first declared task program looked up on `PATH`.
    fn first_path_tool_probe(&self) -> Option<ToolchainProbe> {
        self.config
            .common
            .tasks
            .values()
            .filter_map(|entry| entry.argv.first())
            .find_map(|program| path_tool_probe(program))
    }
}

/// The `--version` probe for a program looked up on `PATH`, or `None` for a
/// program given as a path.
///
/// A path (`scripts/check.sh`, `./tool`, `/opt/bin/tool`) names a specific
/// file, usually a repository script. Toven cannot know whether it accepts
/// `--version`, and running it may have side effects, so it is never executed
/// to probe. A repo script's content is already covered by the task's inputs,
/// and a missing one fails with a typed error when the task spawns it.
fn path_tool_probe(program: &str) -> Option<ToolchainProbe> {
    let is_path = program.contains('/') || program.contains(std::path::MAIN_SEPARATOR);
    (!is_path).then(|| ToolchainProbe::new(program, program, vec!["--version".to_string()]))
}

impl ConfiguredAdapter for CommandAdapter {
    fn discover(&self, request: &DiscoverRequest) -> AppResult<DiscoverResponse> {
        discovery::discover(&self.config, request)
    }

    /// Resolve the toolchain probe with no inference beyond what's declared.
    ///
    /// Precedence: an explicit `[toolchain]` block wins; otherwise the first
    /// declared task program looked up on `PATH` is probed with `--version`. A
    /// program given as a path is never run to probe. The final `command
    /// --version` placeholder is
    /// a label only: [`toolchain_probes_for`](Self::toolchain_probes_for) never
    /// hands it to the prober.
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

        self.first_path_tool_probe().unwrap_or_else(|| {
            ToolchainProbe::new(DEFAULT_TOOL, DEFAULT_TOOL, vec!["--version".to_string()])
        })
    }

    /// Scope the probe to the tool the addressed task actually runs.
    ///
    /// The command ecosystem funnels every declared module into one workspace,
    /// yet each task is an independent tool (`ast-grep` for `structure`,
    /// `mdbook` for `docs-build`). Probing per task — the addressed task's own
    /// `argv[0]`, checked with `--version` — lets each gate surface a typed
    /// missing-tool error for *its* tool without forcing every other command
    /// tool to be installed for an unrelated run. An explicit `[toolchain]`
    /// still wins (it declares one tool for the whole ecosystem). A task whose
    /// program is a path (a repo script) has no probe. An unknown or
    /// program-less task falls back to the first `PATH` tool, or to no probe.
    fn toolchain_probes_for(
        &self,
        _toolchain: &ToolchainTag,
        intent: &TaskIntent,
    ) -> AppResult<Vec<ToolchainProbe>> {
        if self.config.toolchain.is_some() {
            return Ok(vec![self.toolchain_probe()]);
        }
        let probe = self
            .config
            .common
            .tasks
            .get(intent.name())
            .and_then(|entry| entry.argv.first())
            .map_or_else(
                || self.first_path_tool_probe(),
                |program| path_tool_probe(program),
            );
        Ok(probe.into_iter().collect())
    }

    fn run_strategy_default(&self, kind: TaskKind) -> RunStrategy {
        self.config
            .common
            .run_strategy
            .unwrap_or_else(|| tasks::default_run_strategy(kind))
    }

    fn release_target(
        &self,
        _reader: &dyn toven_ports::VcsReader,
    ) -> AppResult<Option<Box<dyn ReleaseAdapter>>> {
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
    use toven_model::ToolchainTag;
    use toven_ports::TaskIntent;

    fn tag() -> ToolchainTag {
        ToolchainTag::new("command")
    }

    fn task_entry(argv: &[&str]) -> TaskEntry {
        TaskEntry {
            kind: None,
            argv: argv.iter().map(ToString::to_string).collect(),
            selector: Vec::new(),
            fan_out: FanOut::PerModule,
            requires: std::collections::BTreeSet::new(),
            depends_on: Vec::new(),
            outputs: Vec::new(),
            workspace_closure: false,
            persistent: false,
            readiness: Readiness::Started,
            readiness_timeout_secs: None,
            cache_args: false,
            cacheable: true,
            fail_if_output: false,
            shared_inputs: Vec::new(),
            modules: Vec::new(),
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

        let structure = adapter
            .toolchain_probes_for(&tag(), &TaskIntent::resolve("structure"))
            .expect("probes");
        assert_eq!(structure.len(), 1);
        assert_eq!(structure[0].program, "ast-grep");
        assert_eq!(structure[0].args, ["--version"]);

        let docs = adapter
            .toolchain_probes_for(&tag(), &TaskIntent::resolve("docs-build"))
            .expect("probes");
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
        let probes = adapter
            .toolchain_probes_for(&tag(), &TaskIntent::resolve("structure"))
            .expect("probes");
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
        let probes = adapter
            .toolchain_probes_for(&tag(), &TaskIntent::resolve("deploy"))
            .expect("probes");
        assert_eq!(probes.len(), 1);
        assert_eq!(probes[0].program, "ast-grep");
    }

    #[test]
    fn a_task_whose_program_is_a_path_is_never_probed() {
        // A repo script may not accept `--version` and may have side effects
        // (regenerating a file), so a path-like `argv[0]` yields no probe.
        let mut common = CommonEcosystemConfig::default();
        common.tasks.insert(
            "module-index".to_string(),
            task_entry(&["scripts/generate-module-index.sh", "--check"]),
        );
        common
            .tasks
            .insert("local".to_string(), task_entry(&["./tool"]));
        common
            .tasks
            .insert("absolute".to_string(), task_entry(&["/opt/bin/tool"]));
        let adapter = CommandAdapter::new(CommandConfig {
            common,
            ..CommandConfig::default()
        });

        for task in ["module-index", "local", "absolute"] {
            let probes = adapter
                .toolchain_probes_for(&tag(), &TaskIntent::resolve(task))
                .expect("probes");
            assert!(probes.is_empty(), "{task} must not be probed: {probes:?}");
        }
    }

    #[test]
    fn the_default_probe_skips_path_programs() {
        let mut common = CommonEcosystemConfig::default();
        common
            .tasks
            .insert("a-script".to_string(), task_entry(&["scripts/check.sh"]));
        common
            .tasks
            .insert("b-docs".to_string(), task_entry(&["mdbook", "build"]));
        let adapter = CommandAdapter::new(CommandConfig {
            common,
            ..CommandConfig::default()
        });

        assert_eq!(adapter.toolchain_probe().program, "mdbook");
        let fallback = adapter
            .toolchain_probes_for(&tag(), &TaskIntent::resolve("deploy"))
            .expect("probes");
        assert_eq!(fallback.len(), 1);
        assert_eq!(fallback[0].program, "mdbook");
    }

    #[test]
    fn an_unknown_task_has_no_probe_when_every_program_is_a_path() {
        let mut common = CommonEcosystemConfig::default();
        common
            .tasks
            .insert("check".to_string(), task_entry(&["scripts/check.sh"]));
        let adapter = CommandAdapter::new(CommandConfig {
            common,
            ..CommandConfig::default()
        });

        assert_eq!(adapter.toolchain_probe().program, "command");
        let probes = adapter
            .toolchain_probes_for(&tag(), &TaskIntent::resolve("deploy"))
            .expect("probes");
        assert!(
            probes.is_empty(),
            "the placeholder is never probed: {probes:?}"
        );
    }

    #[test]
    fn never_offers_a_release_target() {
        let adapter = CommandAdapter::new(CommandConfig::default());
        let reader = toven_testkit::doubles::FakeVcsReader::new();
        assert!(adapter.release_target(&reader).expect("ok").is_none());
    }
}
