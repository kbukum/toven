//! The clap surface: global flags, the reserved-verb command tree, and the
//! per-verb applicability gate.
//!
//! Behavior-shaping flags are defined **once, globally**
//! and accepted before or after the verb; [`gate`] rejects a verb-specific flag
//! used with a verb it does not apply to with a clear, typed error. The
//! argv-first task dispatch itself stays Toven domain — clap only models the
//! reserved verbs and the global flag schema, with bare task names captured as an
//! [`Command::External`] subcommand and re-parsed by [`grammar`](crate::grammar).

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use rskit_errors::{AppError, AppResult};
use toven_engine::vcs::BaselineFlags;

/// Event-sink output format selected by `--output`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lowercase")]
#[non_exhaustive]
pub enum OutputKind {
    /// Human-readable terminal rendering.
    Human,
    /// Machine-parseable JSON-lines Event stream.
    Jsonl,
}

/// Dependency-graph rendering format selected by `--format`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lowercase")]
#[non_exhaustive]
pub enum GraphFormat {
    /// Indented adjacency text.
    Text,
    /// Graphviz DOT.
    Dot,
}

/// The resolved reporter verbosity level (cli-taxonomy `-v`/`-q`).
///
/// Derived from the net of the repeatable `--verbose` and `--quiet` counts: each
/// `-v` raises the level and each `-q` lowers it. The level selects how much of
/// the engine's [`Event`](toven_model::Event) stream the human reporter renders;
/// the machine-parseable JSON-lines stream is unaffected (it always carries every
/// event so consumers see the full record).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Verbosity {
    /// Only the run-level lines (start + the terminal summary).
    Quiet,
    /// The default: run, plan, and terminal per-unit results.
    #[default]
    Normal,
    /// Everything, including per-phase, cache-decision, and unit-lifecycle lines.
    Verbose,
}

impl Verbosity {
    /// Resolve the level from the net of the `verbose` and `quiet` repeat counts.
    ///
    /// A positive net (more `-v` than `-q`) is [`Verbose`](Self::Verbose), a
    /// negative net is [`Quiet`](Self::Quiet), and a balanced net is
    /// [`Normal`](Self::Normal).
    #[must_use]
    pub fn from_counts(verbose: u8, quiet: u8) -> Self {
        match i16::from(verbose) - i16::from(quiet) {
            net if net < 0 => Self::Quiet,
            0 => Self::Normal,
            _ => Self::Verbose,
        }
    }

    /// Resolve the level for execution output, treating `--explain` as one
    /// additional verbosity step so its promised reasoning detail is visible by
    /// default while still allowing explicit quiet flags to reduce output.
    #[must_use]
    pub fn for_execution(verbose: u8, quiet: u8, explain: bool) -> Self {
        Self::from_counts(verbose.saturating_add(u8::from(explain)), quiet)
    }
}

/// The parsed top-level CLI surface: global flags plus the dispatched verb.
#[derive(Debug, Parser)]
#[command(
    name = "toven",
    version,
    about = "Toven — argv-first development and CI task planner for multi-module repositories",
    allow_external_subcommands = true,
    subcommand_required = true,
    arg_required_else_help = true
)]
#[allow(clippy::struct_excessive_bools)]
pub struct Cli {
    /// Path to the `toven.toml` config (otherwise discovered upward from the cwd).
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,
    /// Event-sink format (defaults to the `[toven].report` setting).
    #[arg(long, global = true, value_name = "FORMAT")]
    pub output: Option<OutputKind>,
    /// Run the PLAN cut only, without APPLY.
    #[arg(long, global = true)]
    pub dry_run: bool,
    /// Run the PLAN cut only, with reasoning detail.
    #[arg(long, global = true)]
    pub explain: bool,
    /// Stop scheduling after the first failure (task-APPLY verbs only).
    #[arg(long, global = true)]
    pub fail_fast: bool,
    /// Changed-selection verbs only: override the diff baseline reference
    /// (per-member under a federation; falls back to `[[members]].base_ref` /
    /// `[project].base_ref`).
    #[arg(long, global = true, value_name = "REF")]
    pub base: Option<String>,
    /// Changed-selection verbs only: diff against `merge-base(reference, HEAD)`.
    #[arg(long, global = true)]
    pub merge_base: bool,
    /// Increase reporter verbosity (repeatable; execution verbs only).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,
    /// Decrease reporter verbosity (repeatable; execution verbs only).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub quiet: u8,
    /// Release only: commit/tag a dirty working tree.
    #[arg(long, global = true)]
    pub allow_dirty: bool,
    /// Release only: skip pushing the release commit and tags.
    #[arg(long, global = true)]
    pub no_push: bool,
    /// Generate only: regenerate one `[ecosystems.<id>]` section.
    #[arg(long, global = true, value_name = "ID")]
    pub force: Option<String>,
    /// Generate only: project root to scaffold against.
    #[arg(long, global = true, value_name = "PATH")]
    pub root: Option<PathBuf>,
    /// Generate only: write the rendered `toven.toml` instead of printing it.
    #[arg(long, global = true)]
    pub write: bool,
    /// Graph only: dependency-graph rendering format.
    #[arg(long, global = true, value_name = "FORMAT")]
    pub format: Option<GraphFormat>,
    /// Driver/federation only: provision missing drivers automatically.
    #[arg(long, global = true)]
    pub auto_install: bool,
    /// The dispatched verb (a reserved built-in or a bare task name).
    #[command(subcommand)]
    pub command: Command,
}

/// The reserved built-in verbs plus the argv-first task escape hatch.
#[derive(Debug, Subcommand)]
#[non_exhaustive]
pub enum Command {
    /// Run a task by name (escape hatch for task names that shadow a reserved word).
    Run {
        /// Task name to run.
        task: String,
        /// Passthrough args after `--`, spliced verbatim at each task's `{args}`.
        #[arg(last = true)]
        passthrough: Vec<String>,
    },
    /// Render the PLAN cut for a task (`run <task> --dry-run`).
    Plan {
        /// Task to plan.
        task: String,
    },
    /// Plan and publish a release.
    Release,
    /// Explain the PLAN cut filtered to one module and task.
    Explain {
        /// Module ref (`ecosystem:module`).
        module: String,
        /// Task to explain.
        task: String,
    },
    /// Scaffold or regenerate `toven.toml` sections.
    Generate,
    /// Project the affected-module set for a task.
    Affected {
        /// Task whose blast radius is projected.
        task: String,
    },
    /// List discovered modules.
    #[command(visible_aliases = ["list", "ls"])]
    Modules,
    /// Project the dependency graph (text or dot).
    #[command(visible_alias = "deps")]
    Graph,
    /// Out-of-process driver management.
    Driver {
        /// Driver action.
        #[command(subcommand)]
        action: DriverAction,
    },
    /// Cross-repo federation management.
    Federation {
        /// Federation action.
        #[command(subcommand)]
        action: FederationAction,
    },
    /// Task-cache maintenance.
    Cache {
        /// Cache action.
        #[command(subcommand)]
        action: CacheAction,
    },
    /// A bare top-level token: an argv-first task name plus its trailing args.
    #[command(external_subcommand)]
    External(Vec<String>),
}

/// `toven driver <action>`.
#[derive(Debug, Subcommand)]
#[non_exhaustive]
pub enum DriverAction {
    /// Install an out-of-process driver by id.
    Install {
        /// Driver id to install.
        id: String,
    },
    /// List installed drivers.
    List,
}

/// `toven federation <action>`.
#[derive(Debug, Subcommand)]
#[non_exhaustive]
pub enum FederationAction {
    /// Synchronize federated member repositories.
    Sync,
    /// Report federation status.
    Status,
}

/// `toven cache <action>`.
#[derive(Debug, Subcommand)]
#[non_exhaustive]
pub enum CacheAction {
    /// Summarize the local cache directory.
    Stats,
    /// Remove the local cache directory.
    Clean,
    /// Print the resolved local cache directory.
    Path,
}

impl Cli {
    /// Whether the verb produces a PLAN-only cut (no APPLY half).
    ///
    /// Either an explicit `--dry-run`/`--explain` on an execution verb, or an
    /// introspection verb that is a projection over the PLAN cut by definition.
    #[must_use]
    pub const fn is_plan_only(&self) -> bool {
        self.dry_run || self.explain
    }

    /// The reporter verbosity selected by the global `-v`/`-q` counts.
    #[must_use]
    pub fn verbosity(&self) -> Verbosity {
        Verbosity::for_execution(self.verbose, self.quiet, self.explain)
    }

    /// The CLI-sourced baseline selection (`--base`/`--merge-base`) threaded into
    /// changed-selection. Empty when neither flag is given, so the engine falls
    /// back to the configured `base_ref`.
    #[must_use]
    pub fn baseline_flags(&self) -> BaselineFlags {
        let mut flags = BaselineFlags::new().with_merge_base(self.merge_base);
        if let Some(reference) = &self.base {
            flags = flags.with_base(reference.clone());
        }
        flags
    }
}

/// Reject any verb-specific flag used with a verb it does not apply to.
///
/// # Errors
/// Returns a typed usage error naming the misused flag and the verb it belongs
/// to.
pub fn gate(cli: &Cli) -> AppResult<()> {
    let verb = verb_name(&cli.command);
    let is_release = matches!(cli.command, Command::Release);
    let is_generate = matches!(cli.command, Command::Generate);
    let is_graph = matches!(cli.command, Command::Graph);

    if cli.allow_dirty && !is_release {
        return Err(only_applies("--allow-dirty", "toven release", verb));
    }
    if cli.no_push && !is_release {
        return Err(only_applies("--no-push", "toven release", verb));
    }
    if cli.force.is_some() && !is_generate {
        return Err(only_applies("--force", "toven generate", verb));
    }
    if cli.root.is_some() && !is_generate {
        return Err(only_applies("--root", "toven generate", verb));
    }
    if cli.write && !is_generate {
        return Err(only_applies("--write", "toven generate", verb));
    }
    if cli.format.is_some() && !is_graph {
        return Err(only_applies("--format", "toven graph", verb));
    }
    let accepts_auto_install = matches!(
        cli.command,
        Command::Driver {
            action: DriverAction::List
        } | Command::Federation {
            action: FederationAction::Sync
        }
    );
    if cli.auto_install && !accepts_auto_install {
        return Err(only_applies(
            "--auto-install",
            "toven driver list / toven federation sync",
            verb,
        ));
    }
    if (cli.dry_run || cli.explain || cli.output.is_some())
        && !accepts_execution_flags(&cli.command)
    {
        return Err(AppError::invalid_input(
            "flags",
            format!(
                "execution flags (--dry-run/--explain/--output) do not apply to `toven {verb}`"
            ),
        ));
    }
    // `-v`/`-q` only shape the human reporter, which only the execution verbs
    // build; the introspection/maintenance verbs render their own projection and
    // would silently ignore the flag, so reject it rather than advertise a no-op.
    if (cli.verbose > 0 || cli.quiet > 0) && !accepts_execution_flags(&cli.command) {
        return Err(AppError::invalid_input(
            "flags",
            format!(
                "reporter verbosity (-v/--verbose, -q/--quiet) does not apply to `toven {verb}`"
            ),
        ));
    }
    // `--fail-fast` shapes APPLY scheduling, so it is meaningful only on the
    // task-APPLY verbs. `plan` stops at PLAN and `release` never multiplexes
    // independent units, so the flag is a no-op there and is rejected.
    if cli.fail_fast && !accepts_fail_fast(&cli.command) {
        return Err(AppError::invalid_input(
            "flags",
            format!(
                "`--fail-fast` only applies to task-APPLY verbs (`toven run`/`toven <task>`); it has no effect on `toven {verb}`"
            ),
        ));
    }
    // `--base`/`--merge-base` only shape changed selection, which the execution
    // verbs and `affected` perform; other verbs would silently ignore them.
    if (cli.base.is_some() || cli.merge_base) && !accepts_baseline(&cli.command) {
        let flag = if cli.base.is_some() {
            "--base"
        } else {
            "--merge-base"
        };
        return Err(AppError::invalid_input(
            "flags",
            format!(
                "`{flag}` only applies to changed-selection verbs (`toven run`/`toven plan`/`toven affected`/`toven <task>`); it has no effect on `toven {verb}`"
            ),
        ));
    }
    Ok(())
}

/// Whether `command` is an execution verb that accepts reporter-shaping flags
/// (`--dry-run`/`--explain`/`--output` and the `-v`/`-q` verbosity counts).
const fn accepts_execution_flags(command: &Command) -> bool {
    matches!(
        command,
        Command::Run { .. } | Command::Plan { .. } | Command::Release | Command::External(_)
    )
}

/// Whether `command` is a task-APPLY verb that consumes `--fail-fast`.
///
/// Only the verbs that drive multi-unit APPLY scheduling read fail-fast; `plan`
/// stops at PLAN and `release` runs a single linear pipeline, so the flag is a
/// no-op for them.
const fn accepts_fail_fast(command: &Command) -> bool {
    matches!(command, Command::Run { .. } | Command::External(_))
}

/// Whether `command` performs changed selection and thus consumes the baseline
/// flags (`--base`/`--merge-base`).
const fn accepts_baseline(command: &Command) -> bool {
    matches!(
        command,
        Command::Run { .. }
            | Command::Plan { .. }
            | Command::Affected { .. }
            | Command::External(_)
    )
}

fn only_applies(flag: &str, owner: &str, verb: &str) -> AppError {
    AppError::invalid_input(
        "flags",
        format!("`{flag}` only applies to `{owner}` (used with `toven {verb}`)"),
    )
}

/// The user-facing name of the dispatched verb (for error messages).
fn verb_name(command: &Command) -> &str {
    match command {
        Command::Run { .. } => "run",
        Command::Plan { .. } => "plan",
        Command::Release => "release",
        Command::Explain { .. } => "explain",
        Command::Generate => "generate",
        Command::Affected { .. } => "affected",
        Command::Modules => "modules",
        Command::Graph => "graph",
        Command::Driver { .. } => "driver",
        Command::Federation { .. } => "federation",
        Command::Cache { .. } => "cache",
        Command::External(tokens) => tokens.first().map_or("<task>", String::as_str),
    }
}

#[cfg(test)]
mod tests {
    use super::{Cli, Command};
    use clap::Parser;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("toven").chain(args.iter().copied()))
    }

    #[test]
    fn bare_task_is_captured_as_external() {
        let cli = parse(&["test"]).expect("parses");
        match &cli.command {
            Command::External(tokens) => assert_eq!(tokens, &["test"]),
            other => panic!("expected external, got {other:?}"),
        }
    }

    #[test]
    fn bare_task_trailing_flags_are_captured_as_external_tokens() {
        let cli = parse(&["test", "--dry-run", "--", "--flag"]).expect("parses");
        assert!(!cli.dry_run);
        match &cli.command {
            Command::External(tokens) => {
                assert_eq!(tokens, &["test", "--dry-run", "--", "--flag"]);
            }
            other => panic!("expected external, got {other:?}"),
        }
    }

    #[test]
    fn reserved_verb_dispatches_builtin() {
        let cli = parse(&["plan", "test"]).expect("parses");
        assert!(matches!(cli.command, Command::Plan { .. }));
    }

    #[test]
    fn modules_aliases_resolve() {
        for alias in ["modules", "list", "ls"] {
            let cli = parse(&[alias]).expect("parses");
            assert!(matches!(cli.command, Command::Modules), "alias {alias}");
        }
    }

    #[test]
    fn graph_deps_alias_resolves() {
        assert!(matches!(parse(&["deps"]).unwrap().command, Command::Graph));
    }

    #[test]
    fn global_flag_accepted_before_verb() {
        let cli = parse(&["--dry-run", "plan", "test"]).expect("parses");
        assert!(cli.dry_run);
    }

    #[test]
    fn release_only_flag_on_other_reserved_verb_is_gated() {
        let cli = parse(&["--allow-dirty", "plan", "test"]).expect("parses");
        assert!(super::gate(&cli).is_err());
    }

    #[test]
    fn release_accepts_its_own_flags() {
        let cli = parse(&["--allow-dirty", "--no-push", "release"]).expect("parses");
        assert!(super::gate(&cli).is_ok());
    }

    #[test]
    fn generate_flags_only_on_generate() {
        for args in [
            ["--write", "generate"].as_slice(),
            ["--root", "/tmp", "generate"].as_slice(),
            ["--force", "rust", "generate"].as_slice(),
        ] {
            assert!(super::gate(&parse(args).unwrap()).is_ok(), "{args:?}");
        }
        for args in [
            ["--write", "plan", "test"].as_slice(),
            ["--root", "/tmp", "modules"].as_slice(),
            ["--force", "rust", "graph"].as_slice(),
        ] {
            assert!(super::gate(&parse(args).unwrap()).is_err(), "{args:?}");
        }
    }

    #[test]
    fn graph_format_only_on_graph() {
        assert!(super::gate(&parse(&["--format", "dot", "graph"]).unwrap()).is_ok());
        assert!(super::gate(&parse(&["--format", "dot", "plan", "t"]).unwrap()).is_err());
    }

    #[test]
    fn execution_flags_rejected_on_cache() {
        assert!(super::gate(&parse(&["--dry-run", "cache", "path"]).unwrap()).is_err());
    }

    #[test]
    fn execution_flags_rejected_on_introspection_verbs() {
        for args in [
            ["--dry-run", "explain", "rust:app", "test"].as_slice(),
            ["--explain", "affected", "test"].as_slice(),
            ["--fail-fast", "modules"].as_slice(),
            ["--output", "jsonl", "graph"].as_slice(),
        ] {
            let cli = parse(args).expect("parses");
            assert!(super::gate(&cli).is_err(), "{args:?}");
        }
    }

    #[test]
    fn execution_flags_accepted_on_execution_verbs() {
        for args in [
            ["--dry-run", "run", "test"].as_slice(),
            ["--explain", "plan", "test"].as_slice(),
            ["--output", "jsonl", "release"].as_slice(),
            ["--fail-fast", "test"].as_slice(),
        ] {
            let cli = parse(args).expect("parses");
            assert!(super::gate(&cli).is_ok(), "{args:?}");
        }
    }

    #[test]
    fn fail_fast_rejected_on_non_apply_verbs() {
        // `--fail-fast` shapes APPLY scheduling, so it is a no-op on `plan`
        // (PLAN-only) and `release` (single linear pipeline) and rejected there.
        for args in [
            ["--fail-fast", "plan", "test"].as_slice(),
            ["--fail-fast", "release"].as_slice(),
        ] {
            let cli = parse(args).expect("parses");
            assert!(super::gate(&cli).is_err(), "{args:?}");
        }
    }

    #[test]
    fn fail_fast_accepted_on_task_apply_verbs() {
        for args in [
            ["--fail-fast", "run", "test"].as_slice(),
            ["--fail-fast", "test"].as_slice(),
        ] {
            let cli = parse(args).expect("parses");
            assert!(super::gate(&cli).is_ok(), "{args:?}");
        }
    }

    #[test]
    fn verbosity_rejected_on_non_execution_verbs() {
        // `-v`/`-q` only shape the human reporter the execution verbs build; the
        // introspection/maintenance verbs ignore it, so it is rejected there.
        for args in [
            ["-v", "modules"].as_slice(),
            ["--verbose", "graph"].as_slice(),
            ["-q", "cache", "path"].as_slice(),
            ["--quiet", "explain", "rust:app", "test"].as_slice(),
        ] {
            let cli = parse(args).expect("parses");
            assert!(super::gate(&cli).is_err(), "{args:?}");
        }
    }

    #[test]
    fn verbosity_accepted_on_execution_verbs() {
        for args in [
            ["-v", "run", "test"].as_slice(),
            ["-q", "plan", "test"].as_slice(),
            ["--verbose", "release"].as_slice(),
            ["-vv", "test"].as_slice(),
        ] {
            let cli = parse(args).expect("parses");
            assert!(super::gate(&cli).is_ok(), "{args:?}");
        }
    }

    #[test]
    fn baseline_flags_accepted_on_changed_selection_verbs() {
        for args in [
            ["--base", "origin/main", "run", "test"].as_slice(),
            ["--base", "origin/main", "plan", "test"].as_slice(),
            ["--merge-base", "affected", "test"].as_slice(),
            ["--base", "origin/main", "test"].as_slice(),
        ] {
            let cli = parse(args).expect("parses");
            assert!(super::gate(&cli).is_ok(), "{args:?}");
        }
    }

    #[test]
    fn baseline_flags_rejected_on_other_verbs() {
        for args in [
            ["--base", "origin/main", "release"].as_slice(),
            ["--merge-base", "modules"].as_slice(),
            ["--base", "origin/main", "graph"].as_slice(),
            ["--merge-base", "explain", "rust:app", "test"].as_slice(),
        ] {
            let cli = parse(args).expect("parses");
            assert!(super::gate(&cli).is_err(), "{args:?}");
        }
    }

    #[test]
    fn baseline_flags_resolve_into_engine_flags() {
        let cli =
            parse(&["--base", "origin/main", "--merge-base", "affected", "test"]).expect("parses");
        let flags = cli.baseline_flags();
        assert_eq!(flags.base.as_deref(), Some("origin/main"));
        assert!(flags.merge_base);
    }

    #[test]
    fn verbosity_resolves_from_the_net_of_verbose_and_quiet() {
        use super::Verbosity;
        assert_eq!(Verbosity::from_counts(0, 0), Verbosity::Normal);
        assert_eq!(Verbosity::from_counts(2, 0), Verbosity::Verbose);
        assert_eq!(Verbosity::from_counts(0, 1), Verbosity::Quiet);
        // A balanced net is normal; an excess on either side wins.
        assert_eq!(Verbosity::from_counts(1, 1), Verbosity::Normal);
        assert_eq!(Verbosity::from_counts(2, 1), Verbosity::Verbose);
        assert_eq!(Verbosity::from_counts(1, 2), Verbosity::Quiet);
    }

    #[test]
    fn explain_raises_execution_verbosity() {
        use super::Verbosity;
        assert_eq!(Verbosity::for_execution(0, 0, true), Verbosity::Verbose);
        assert_eq!(Verbosity::for_execution(0, 1, true), Verbosity::Normal);
    }
}
