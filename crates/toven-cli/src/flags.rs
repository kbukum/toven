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
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use rskit_errors::{AppError, AppResult};
use rskit_util::time::parse_duration;
use toven_engine::vcs::BaselineFlags;

/// Default trailing-edge debounce window (ms) for `--watch` when
/// `--watch-debounce-ms` is not given.
pub const DEFAULT_WATCH_DEBOUNCE_MS: u64 = 200;

/// Top-level `--help` examples covering the most common argv-first workflows.
const TOP_LEVEL_EXAMPLES: &str = "\
Examples:
  toven test                 Run the `test` task across the affected modules
  toven build --dry-run      Show the PLAN cut without executing anything
  toven test -- --nocapture  Splice passthrough args at each task's `{args}`
  toven modules              List the discovered modules
  toven tasks                List every runnable task, per ecosystem
  toven graph --format dot   Emit the dependency graph as Graphviz DOT
  toven completions zsh      Print a zsh completion script

Any non-reserved token is an argv-first task name. Run `toven <command> --help`
for command-specific examples.";

/// `run` verb examples (the argv-first escape hatch for shadowed task names).
const RUN_EXAMPLES: &str = "\
Examples:
  toven run test             Run a task whose name shadows a reserved verb
  toven run test -- -q       Forward `-q` to the task at its `{args}` slot";

/// `plan` verb examples.
const PLAN_EXAMPLES: &str = "\
Examples:
  toven plan build           Show the PLAN cut (waves + units) for `build`";

/// `explain` verb examples.
const EXPLAIN_EXAMPLES: &str = "\
Examples:
  toven explain test                Explain every module's unit for `test`
  toven explain test --module core  Explain the unit(s) for one module + task";

/// `affected` verb examples.
const AFFECTED_EXAMPLES: &str = "\
Examples:
  toven affected test        Project the modules a `test` run would touch";

/// `coverage` verb examples.
const COVERAGE_EXAMPLES: &str = "\
Examples:
  toven coverage                 Run coverage, aggregate profiles, and gate
  toven coverage --line 90       Override the line floor for this run
  toven coverage --enforcement advisory  Report shortfalls without failing";

/// `modules` verb examples.
const MODULES_EXAMPLES: &str = "\
Examples:
  toven modules              List every discovered module";

/// `graph` verb examples.
const GRAPH_EXAMPLES: &str = "\
Examples:
  toven graph                Print the dependency graph as indented text
  toven graph --format dot   Emit Graphviz DOT for `dot -Tsvg`";

/// `tasks` verb examples.
const TASKS_EXAMPLES: &str = "\
Examples:
  toven tasks                List every runnable task, per ecosystem
  toven tasks format         Show one task's argv template and inputs";

/// `completions` verb examples.
const COMPLETIONS_EXAMPLES: &str = "\
Examples:
  toven completions zsh > _toven      Install zsh completions
  source <(toven completions bash)    Load bash completions for this shell";

/// `init` verb examples.
const INIT_EXAMPLES: &str = "\
Examples:
  toven init                 Detect ecosystems and write a `toven.toml` for the current repo
  toven init --non-interactive   Take questionnaire defaults with no prompts (CI)
  toven init --print         Preview the rendered `toven.toml` on stdout without writing
  toven init --force rust    Regenerate just the `[ecosystems.rust]` block";

/// Parse a `--timeout` duration string (e.g. `30s`, `5m`) into a [`Duration`].
///
/// A clap `value_parser`, so it also backs the trailing-token path via
/// [`parse_timeout`](crate::grammar::parse_timeout): both dispatch routes reject
/// the same malformed values with the same message. Rejects a zero or unparseable
/// duration — a bound of zero would fail every unit immediately, which is never
/// what the user means.
pub(crate) fn parse_duration_arg(value: &str) -> Result<Duration, String> {
    match parse_duration(value) {
        Some(duration) if !duration.is_zero() => Ok(duration),
        Some(_) => Err(format!(
            "`--timeout` must be greater than zero (got `{value}`)"
        )),
        None => Err(format!(
            "`--timeout` requires a duration like `30s`, `5m`, or `2h` (got `{value}`)"
        )),
    }
}

/// Parse `--jobs`/`-j`: a positive concurrency ceiling.
///
/// Rejects zero — a ceiling of zero would schedule nothing, which is never what
/// the user means; `--jobs 1` is the way to force strictly serial execution.
pub(crate) fn parse_jobs_arg(value: &str) -> Result<usize, String> {
    match value.parse::<usize>() {
        Ok(0) => Err("`--jobs` must be at least 1 (use `--jobs 1` for serial)".to_owned()),
        Ok(jobs) => Ok(jobs),
        Err(error) => Err(format!(
            "`--jobs` requires a positive integer (got `{value}`): {error}"
        )),
    }
}

/// Parse a coverage threshold flag (`--line`/`--function`/`--region`/
/// `--changed-line`): a percentage in `0..=100`.
pub(crate) fn parse_percentage_arg(value: &str) -> Result<f64, String> {
    match value.parse::<f64>() {
        Ok(pct) if (0.0..=100.0).contains(&pct) => Ok(pct),
        Ok(pct) => Err(format!(
            "a coverage threshold must be a percentage in 0..=100 (got `{pct}`)"
        )),
        Err(error) => Err(format!(
            "a coverage threshold requires a number like `90` or `85.5` (got `{value}`): {error}"
        )),
    }
}

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

/// How live per-unit output is rendered on a terminal, selected by `--view`.
///
/// Mirrors the engine's [`ViewMode`](toven_engine::config::ViewMode) so a flag
/// and the `[toven].view` document setting resolve to the same rendering; the
/// flag wins when both are present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ViewMode {
    /// Pick the richest shape the environment supports (default).
    Auto,
    /// One live, content-sized tile per in-flight unit in a single terminal.
    Tiles,
    /// One multiplexer pane per unit (opt-in; requires a supported multiplexer).
    Panes,
    /// A single linear stream, log-friendly (each unit flushed as one block).
    Stream,
}

impl From<ViewMode> for toven_engine::config::ViewMode {
    fn from(view: ViewMode) -> Self {
        match view {
            ViewMode::Auto => Self::Auto,
            ViewMode::Tiles => Self::Tiles,
            ViewMode::Panes => Self::Panes,
            ViewMode::Stream => Self::Stream,
        }
    }
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

/// Coverage enforcement mode selected by `--enforcement`, mirroring
/// [`Enforcement`](toven_ports::Enforcement) so the flag overrides the
/// `[…coverage].enforcement` config default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lowercase")]
#[non_exhaustive]
pub enum EnforcementArg {
    /// Fail the gate closed on a below-threshold dimension.
    Block,
    /// Measure and report the shortfall without failing.
    Advisory,
}

impl From<EnforcementArg> for toven_ports::Enforcement {
    fn from(arg: EnforcementArg) -> Self {
        match arg {
            EnforcementArg::Block => Self::Block,
            EnforcementArg::Advisory => Self::Advisory,
        }
    }
}

/// When to emit ANSI color in the human reporter, selected by `--color`.
///
/// Maps to rskit's [`ColorChoice`](rskit_cli::ColorChoice); the `NO_COLOR`
/// environment variable always overrides an explicit `always`, following the
/// [`NO_COLOR` standard](https://no-color.org).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
#[value(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ColorWhen {
    /// Color only when stderr is a terminal and `NO_COLOR` is unset (default).
    #[default]
    Auto,
    /// Force color on (still overridden by `NO_COLOR`).
    Always,
    /// Force color off.
    Never,
}

impl From<ColorWhen> for rskit_cli::ColorChoice {
    fn from(when: ColorWhen) -> Self {
        match when {
            ColorWhen::Auto => Self::Auto,
            ColorWhen::Always => Self::Always,
            ColorWhen::Never => Self::Never,
        }
    }
}

impl std::fmt::Display for ColorWhen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::Auto => "auto",
            Self::Always => "always",
            Self::Never => "never",
        };
        f.write_str(label)
    }
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
    after_help = TOP_LEVEL_EXAMPLES,
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
    /// When to colorize the human reporter: `auto` (default), `always`, or
    /// `never`. `NO_COLOR` always wins over `always`. Only the execution verbs
    /// build a human reporter, so an explicit `--color` is rejected elsewhere.
    #[arg(long, global = true, value_name = "WHEN")]
    pub color: Option<ColorWhen>,
    /// How live per-unit output renders while a task runs: `auto` (default),
    /// `tiles`, `panes`, or `stream`. Applies only to the task-APPLY verbs
    /// (`toven run` / `toven <task>`); overrides the `[toven].view` setting.
    /// Redirected, piped, `--output jsonl`, and non-interactive runs always use
    /// the linear `stream` shape regardless of this flag.
    #[arg(long, global = true, value_name = "MODE", help_heading = "Execution")]
    pub view: Option<ViewMode>,
    /// Run the PLAN cut only, without APPLY.
    #[arg(long, global = true, help_heading = "Execution")]
    pub dry_run: bool,
    /// Run the PLAN cut only, with reasoning detail.
    #[arg(long, global = true, help_heading = "Execution")]
    pub explain: bool,
    /// Stop scheduling after the first failure (task-APPLY verbs only).
    #[arg(long, global = true, help_heading = "Execution")]
    pub fail_fast: bool,
    /// Execution verbs only: bypass the task cache (every unit re-runs; records
    /// are neither read nor written).
    #[arg(long, global = true, help_heading = "Execution")]
    pub no_cache: bool,
    /// Execution verbs only: ignore cached results and re-run every unit, but
    /// still write the fresh results back (distinct from `--no-cache`, which
    /// neither reads nor writes). Mutually exclusive with `--no-cache`.
    #[arg(long, global = true, help_heading = "Execution")]
    pub refresh: bool,
    /// Task-APPLY verbs only: bound how long any single execution unit may run
    /// before it is cooperatively cancelled and reported as a timeout failure
    /// (duration string, e.g. `30s`, `5m`).
    #[arg(long, global = true, value_name = "DURATION", value_parser = parse_duration_arg, help_heading = "Execution")]
    pub timeout: Option<Duration>,
    /// Task-APPLY verbs only: cap how many units run concurrently, overriding the
    /// `[toven].max_parallel` setting. `--jobs 1` forces strictly serial
    /// execution (one unit at a time), which streams each unit's output inline
    /// as a single continuous log instead of buffered per-unit blocks.
    #[arg(long, short = 'j', global = true, value_name = "N", value_parser = parse_jobs_arg, help_heading = "Execution")]
    pub jobs: Option<usize>,
    /// Changed-selection verbs only: override the diff baseline reference
    /// (per-member under a federation; falls back to `[[members]].base_ref` /
    /// `[project].base_ref`).
    #[arg(long, global = true, value_name = "REF", help_heading = "Selection")]
    pub base: Option<String>,
    /// Changed-selection verbs only: diff against `merge-base(reference, HEAD)`.
    #[arg(long, global = true, help_heading = "Selection")]
    pub merge_base: bool,
    /// Execution/affected/explain verbs and bare tasks: activate modules by
    /// selector — a bare name (`core`), `ecosystem:name` (`rust:core`),
    /// `workspace/name` (`backend/api`), or a glob (`rust:*`, `rskit-*`) —
    /// bypassing changed-selection (repeatable).
    #[arg(
        long = "module",
        global = true,
        value_name = "SELECTOR",
        help_heading = "Selection"
    )]
    pub module: Vec<String>,
    /// Execution/affected/explain verbs and bare tasks: activate every module
    /// owned by a workspace, by id or glob (`backend`, `rust:contrib`,
    /// `backend*`), bypassing changed-selection (repeatable).
    #[arg(
        long = "workspace",
        global = true,
        value_name = "SELECTOR",
        help_heading = "Selection"
    )]
    pub workspace: Vec<String>,
    /// Execution/affected/explain verbs and bare tasks: with `--module`/
    /// `--workspace`, also activate the reverse-dependents closure (everything
    /// that depends on the selection).
    #[arg(
        long = "dependents",
        alias = "with-dependents",
        global = true,
        help_heading = "Selection"
    )]
    pub with_dependents: bool,
    /// Execution/affected/explain verbs and bare tasks: with `--module`/
    /// `--workspace`, also activate the forward-dependencies closure (everything
    /// the selection needs).
    #[arg(long = "dependencies", global = true, help_heading = "Selection")]
    pub with_dependencies: bool,
    /// Task-APPLY verbs only: keep running, re-executing the affected subgraph
    /// each time a watched source file changes (Ctrl+C exits).
    #[arg(long, global = true, help_heading = "Execution")]
    pub watch: bool,
    /// Watch only: trailing-edge debounce window, in milliseconds, for
    /// coalescing a burst of filesystem events into one rerun (default 200).
    #[arg(long, global = true, value_name = "MS", help_heading = "Execution")]
    pub watch_debounce_ms: Option<u64>,
    /// Increase reporter verbosity (repeatable; execution verbs only).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,
    /// Decrease reporter verbosity (repeatable; execution verbs only).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub quiet: u8,
    /// Release only: commit/tag a dirty working tree.
    #[arg(long, global = true, help_heading = "Release")]
    pub allow_dirty: bool,
    /// Release only: skip pushing the release commit and tags.
    #[arg(long, global = true, help_heading = "Release")]
    pub no_push: bool,
    /// Release tag/publish only: force `<module>` to bump at the patch level
    /// (repeatable). Highest precedence with the other level flags and
    /// `--set-version`; a module named in two level flags or a level flag plus
    /// `--set-version` is a usage error.
    #[arg(
        long = "patch",
        global = true,
        value_name = "MODULE",
        help_heading = "Release"
    )]
    pub patch: Vec<String>,
    /// Release tag/publish only: force `<module>` to bump at the minor level
    /// (repeatable).
    #[arg(
        long = "minor",
        global = true,
        value_name = "MODULE",
        help_heading = "Release"
    )]
    pub minor: Vec<String>,
    /// Release tag/publish only: force `<module>` to bump at the major level
    /// (repeatable).
    #[arg(
        long = "major",
        global = true,
        value_name = "MODULE",
        help_heading = "Release"
    )]
    pub major: Vec<String>,
    /// Release tag/publish only: pin `<module>=<x.y.z>` to an explicit target
    /// version (repeatable).
    #[arg(
        long = "set-version",
        global = true,
        value_name = "MODULE=VERSION",
        help_heading = "Release"
    )]
    pub set_version: Vec<String>,
    /// Release tag/publish only: cut a prerelease on a configured channel
    /// (`rc`/`alpha`/`beta`).
    #[arg(
        long = "pre",
        global = true,
        value_name = "CHANNEL",
        help_heading = "Release"
    )]
    pub pre: Option<String>,
    /// Release tag/publish only: skip registry `published_versions` lookups and
    /// anchor idempotency on the release tag only.
    #[arg(long, global = true, help_heading = "Release")]
    pub offline: bool,
    /// Release sbom/depgraphs only: directory to write generated artifacts into
    /// (created if absent; defaults to `target/toven/release`).
    #[arg(long, global = true, value_name = "PATH", help_heading = "Release")]
    pub out_dir: Option<PathBuf>,
    /// Coverage only: override the absolute line-coverage floor for this run
    /// (percentage, `0..=100`); wins over the `[…coverage].line` config default.
    #[arg(long, global = true, value_name = "PCT", value_parser = parse_percentage_arg, help_heading = "Coverage")]
    pub line: Option<f64>,
    /// Coverage only: override the function-coverage floor for this run
    /// (percentage; gated only where the ecosystem measures functions).
    #[arg(long, global = true, value_name = "PCT", value_parser = parse_percentage_arg, help_heading = "Coverage")]
    pub function: Option<f64>,
    /// Coverage only: override the region-coverage floor for this run
    /// (percentage; gated only where the ecosystem measures regions).
    #[arg(long, global = true, value_name = "PCT", value_parser = parse_percentage_arg, help_heading = "Coverage")]
    pub region: Option<f64>,
    /// Coverage only: override the changed-lines floor for this run (percentage;
    /// applied to changed files under a changed selection).
    #[arg(long = "changed-line", global = true, value_name = "PCT", value_parser = parse_percentage_arg, help_heading = "Coverage")]
    pub changed_line: Option<f64>,
    /// Coverage only: override how a below-threshold verdict is enforced
    /// (`block` fails the gate; `advisory` reports without failing).
    #[arg(long, global = true, value_name = "MODE", help_heading = "Coverage")]
    pub enforcement: Option<EnforcementArg>,
    /// Init only: regenerate one `[ecosystems.<id>]` section.
    #[arg(long, global = true, value_name = "ID", help_heading = "Init")]
    pub force: Option<String>,
    /// Init only: project root to onboard against.
    #[arg(long, global = true, value_name = "PATH", help_heading = "Init")]
    pub root: Option<PathBuf>,
    /// Init only: answer the wizard non-interactively (take questionnaire
    /// defaults, never prompt).
    #[arg(
        long = "non-interactive",
        visible_alias = "yes",
        global = true,
        help_heading = "Init"
    )]
    pub non_interactive: bool,
    /// Init only: render the `toven.toml` to stdout and write nothing.
    #[arg(long, global = true, help_heading = "Init")]
    pub print: bool,
    /// Graph only: dependency-graph rendering format.
    #[arg(long, global = true, value_name = "FORMAT", help_heading = "Graph")]
    pub format: Option<GraphFormat>,
    /// Driver/federation only: provision missing drivers automatically.
    #[arg(long, global = true, help_heading = "Driver")]
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
    #[command(after_long_help = RUN_EXAMPLES)]
    Run {
        /// Task name to run.
        task: String,
        /// Passthrough args after `--`, spliced verbatim at each task's `{args}`.
        #[arg(last = true)]
        passthrough: Vec<String>,
    },
    /// Render the PLAN cut for a task (`run <task> --dry-run`).
    #[command(after_long_help = PLAN_EXAMPLES)]
    Plan {
        /// Task to plan.
        task: String,
    },
    /// Plan, inspect, and publish a release through its lifecycle actions.
    Release {
        /// Release lifecycle action.
        #[command(subcommand)]
        action: ReleaseAction,
    },
    /// Run the coverage task, aggregate the emitted profiles per module, and gate
    /// them against the resolved `[…coverage]` thresholds.
    #[command(after_long_help = COVERAGE_EXAMPLES)]
    Coverage,
    /// Explain the PLAN cut for a task, optionally filtered to a `--module`
    /// selection.
    #[command(after_long_help = EXPLAIN_EXAMPLES)]
    Explain {
        /// Task to explain.
        task: String,
    },
    /// Detect ecosystems and write (or preview) a `toven.toml` via the wizard.
    #[command(after_long_help = INIT_EXAMPLES)]
    Init,
    /// Project the affected-module set for a task.
    #[command(after_long_help = AFFECTED_EXAMPLES)]
    Affected {
        /// Task whose blast radius is projected.
        task: String,
    },
    /// List discovered modules.
    #[command(visible_aliases = ["list", "ls"], after_long_help = MODULES_EXAMPLES)]
    Modules,
    /// Project the dependency graph (text or dot).
    #[command(visible_alias = "deps", after_long_help = GRAPH_EXAMPLES)]
    Graph,
    /// List the runnable tasks resolved for each ecosystem.
    #[command(after_long_help = TASKS_EXAMPLES)]
    Tasks {
        /// Optional task name to show in detail (argv template, inputs).
        name: Option<String>,
    },
    /// Print a shell completion script (`bash`/`zsh`/`fish`/`powershell`/`elvish`).
    #[command(after_long_help = COMPLETIONS_EXAMPLES)]
    Completions {
        /// Target shell for the emitted completion script.
        shell: clap_complete::Shell,
    },
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

/// `toven release <action>`.
///
/// A reviewable release lifecycle: read-only projections (`plan`, `status`) that
/// never mutate, and mutating actions (`tag`, `publish`) that run the release
/// pipeline. `tag` stops after the release commit/tag/push; `publish` continues
/// to the registry. `--dry-run` turns `publish` into a no-mutation rehearsal that
/// reports the resolved publish order and per-module verdicts.
#[derive(Debug, Clone, Copy, Subcommand)]
#[non_exhaustive]
pub enum ReleaseAction {
    /// Show the release PLAN cut — bumped versions, changelog, and publish order
    /// — without mutating anything.
    Plan,
    /// Show each module's declared version versus what is published and tagged
    /// (read-only).
    Status,
    /// Cut the release: bump manifests, commit, tag, and push — without
    /// publishing to the registry.
    Tag,
    /// Run the full release pipeline (commit, tag, push, publish); `--dry-run`
    /// rehearses the publish order and per-module would-publish/already-published
    /// verdicts without mutating anything.
    Publish,
    /// Evaluate the fail-closed release-readiness preflight (configured
    /// go/no-go checks) without mutating anything.
    Readiness,
    /// Generate a CycloneDX SBOM per releasable module under `--out-dir`
    /// (read-only).
    #[allow(clippy::doc_markdown)]
    Sbom,
    /// Render the dependency graph to a DOT artifact under `--out-dir`
    /// (read-only).
    Depgraphs,
}

impl ReleaseAction {
    /// The action's canonical name (for error messages and help).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Status => "status",
            Self::Tag => "tag",
            Self::Publish => "publish",
            Self::Readiness => "readiness",
            Self::Sbom => "sbom",
            Self::Depgraphs => "depgraphs",
        }
    }

    /// Whether the action mutates history/registry (accepts `--allow-dirty` /
    /// `--no-push`).
    #[must_use]
    pub const fn is_mutating(self) -> bool {
        matches!(self, Self::Tag | Self::Publish)
    }

    /// Whether the action is a read-only projection (`plan` / `status` /
    /// `readiness` / `sbom` / `depgraphs`).
    #[must_use]
    pub const fn is_projection(self) -> bool {
        matches!(
            self,
            Self::Plan | Self::Status | Self::Readiness | Self::Sbom | Self::Depgraphs
        )
    }

    /// Whether the action writes artifacts under `--out-dir` (`sbom` /
    /// `depgraphs`).
    #[must_use]
    pub const fn writes_artifacts(self) -> bool {
        matches!(self, Self::Sbom | Self::Depgraphs)
    }

    /// Whether `--dry-run` is meaningful for the action: `plan` (already a
    /// projection) and the rehearsable `publish` pipeline.
    #[must_use]
    pub const fn accepts_dry_run(self) -> bool {
        matches!(self, Self::Plan | Self::Publish)
    }
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
        if self.dry_run || self.explain {
            return true;
        }
        matches!(
            self.command,
            Command::Release { action } if action.is_projection()
        )
    }

    /// The reporter verbosity selected by the global `-v`/`-q` counts.
    #[must_use]
    pub fn verbosity(&self) -> Verbosity {
        Verbosity::for_execution(self.verbose, self.quiet, self.explain)
    }

    /// The color policy applied to the human reporter, defaulting to `auto` when
    /// `--color` is not given.
    #[must_use]
    pub fn color_choice(&self) -> ColorWhen {
        self.color.unwrap_or(ColorWhen::Auto)
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
    let verb_owned = verb_name(&cli.command);
    let verb = verb_owned.as_str();
    let mutating_release = release_action(&cli.command).is_some_and(ReleaseAction::is_mutating);
    let is_init = matches!(cli.command, Command::Init);
    let is_graph = matches!(cli.command, Command::Graph);
    let is_coverage = matches!(cli.command, Command::Coverage);

    // `--allow-dirty`/`--no-push` bypass a release guardrail, so they belong only
    // to the mutating release actions; the read-only projections (`release plan`/
    // `release status`) never touch history, and no other verb releases at all.
    if cli.allow_dirty && !mutating_release {
        return Err(only_applies(
            "--allow-dirty",
            "toven release tag/publish",
            verb,
        ));
    }
    if cli.no_push && !mutating_release {
        return Err(only_applies("--no-push", "toven release tag/publish", verb));
    }
    gate_out_dir_flag(cli, verb)?;
    gate_bump_flags(cli, verb, mutating_release)?;
    gate_init_flags(cli, verb, is_init)?;
    gate_coverage_flags(cli, verb, is_coverage)?;
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
    // `--dry-run` is a PLAN cut for task verbs and a no-mutation rehearsal for
    // `release publish` (and a no-op on the already-dry `release plan`);
    // `--explain` adds task-planning reasoning detail. Neither has meaning on the
    // introspection/maintenance verbs, and `--dry-run` is a no-op on the mutating
    // `release tag` and read-only `release status`, so reject it there.
    if cli.dry_run && !accepts_dry_run(&cli.command) {
        return Err(AppError::invalid_input(
            "flags",
            format!("`--dry-run` does not apply to `toven {verb}`"),
        ));
    }
    if cli.explain && !accepts_explain(&cli.command) {
        return Err(AppError::invalid_input(
            "flags",
            format!("`--explain` does not apply to `toven {verb}`"),
        ));
    }
    // `--output` selects the event-sink/projection format; the execution verbs,
    // the release lifecycle actions, and the `tasks`/`modules` listings render a
    // chooseable projection, but the other verbs print their own fixed rendering.
    if cli.output.is_some() && !accepts_output_format(&cli.command) {
        return Err(AppError::invalid_input(
            "flags",
            format!("`--output` does not apply to `toven {verb}`"),
        ));
    }
    // `-v`/`-q` only shape the human run reporter, which the execution verbs and
    // the mutating release actions build; the introspection/maintenance verbs and
    // the read-only release projections render their own projection and would
    // silently ignore the flag, so reject it rather than advertise a no-op.
    if (cli.verbose > 0 || cli.quiet > 0) && !accepts_reporter_shaping(&cli.command) {
        return Err(AppError::invalid_input(
            "flags",
            format!(
                "reporter verbosity (-v/--verbose, -q/--quiet) does not apply to `toven {verb}`"
            ),
        ));
    }
    // `--color` shapes the same human reporter as `-v`/`-q`; only the verbs that
    // build it consume it. An explicit `--color` elsewhere would be a silent
    // no-op, so reject it rather than advertise one.
    if cli.color.is_some() && !accepts_reporter_shaping(&cli.command) {
        return Err(AppError::invalid_input(
            "flags",
            format!("`--color` does not apply to `toven {verb}`"),
        ));
    }
    // `--view` shapes only the live APPLY output rendering, so it is meaningful
    // only on the task-APPLY verbs that stream child output. On `plan`
    // (PLAN-only), `release`, and every introspection/maintenance verb it would
    // be a silent no-op, so reject it rather than advertise one.
    gate_view_flag(cli, verb)?;
    // `--fail-fast` shapes APPLY scheduling, so it is meaningful only on the
    // task-APPLY verbs. `plan` stops at PLAN and `release` never multiplexes
    // independent units, so the flag is a no-op there and is rejected.
    reject_apply_only_flag(cli.fail_fast, "--fail-fast", &cli.command, verb)?;
    // `--no-cache` shapes the PLAN cache verdict, so it is meaningful only on the
    // execution verbs that build a cache-aware `PlanRequest` (`run`/`plan`/a bare
    // task). `release` runs its own pipeline without the task cache, so the flag
    // is a no-op there and is rejected.
    if cli.no_cache && !accepts_cache_mode(&cli.command) {
        return Err(AppError::invalid_input(
            "flags",
            format!(
                "`--no-cache` only applies to task verbs (`toven run`/`toven plan`/`toven <task>`); it has no effect on `toven {verb}`"
            ),
        ));
    }
    // `--refresh` shapes the same PLAN cache verdict as `--no-cache` (force a
    // re-run) and so is gated to the same cache-aware execution verbs.
    if cli.refresh && !accepts_cache_mode(&cli.command) {
        return Err(AppError::invalid_input(
            "flags",
            format!(
                "`--refresh` only applies to task verbs (`toven run`/`toven plan`/`toven <task>`); it has no effect on `toven {verb}`"
            ),
        ));
    }
    // `--refresh` (force re-run, keep writing) and `--no-cache` (neither read nor
    // write) are contradictory cache policies; reject the combination rather than
    // silently letting one win.
    if cli.refresh && cli.no_cache {
        return Err(refresh_no_cache_conflict());
    }
    // `--timeout` bounds APPLY execution and `--jobs` caps its concurrency, so —
    // like `--fail-fast`/`--watch` — both are meaningful only on the task-APPLY
    // verbs that actually run units.
    reject_apply_only_flag(cli.timeout.is_some(), "--timeout", &cli.command, verb)?;
    reject_apply_only_flag(cli.jobs.is_some(), "--jobs", &cli.command, verb)?;
    gate_watch_flags(cli, verb)?;
    // `--base`/`--merge-base` only shape changed selection, and
    // `--module`/`--workspace`/`--with-dependents` shape explicit selection —
    // both belong to the same selection verbs; other verbs would ignore them.
    gate_selection_flags(cli, verb)?;
    Ok(())
}

/// Reject an APPLY-only flag (`--fail-fast`/`--timeout`/`--jobs`) on any verb
/// that never runs units; elsewhere the flag would be a silent no-op.
fn reject_apply_only_flag(
    present: bool,
    flag: &str,
    command: &Command,
    verb: &str,
) -> AppResult<()> {
    if present && !accepts_fail_fast(command) {
        return Err(AppError::invalid_input(
            "flags",
            format!(
                "`{flag}` only applies to task-APPLY verbs (`toven run`/`toven <task>`); it has no effect on `toven {verb}`"
            ),
        ));
    }
    Ok(())
}

/// Reject `--view` on any verb other than the task-APPLY verbs that stream
/// live child output; elsewhere it is a silent no-op.
fn gate_view_flag(cli: &Cli, verb: &str) -> AppResult<()> {
    if cli.view.is_some() && !accepts_fail_fast(&cli.command) {
        return Err(AppError::invalid_input(
            "flags",
            format!(
                "`--view` only applies to task-APPLY verbs (`toven run`/`toven <task>`); it has no effect on `toven {verb}`"
            ),
        ));
    }
    Ok(())
}

/// Reject `--out-dir` on anything but the artifact-writing release actions
/// (`release sbom`/`release depgraphs`); elsewhere it has no target directory.
fn gate_out_dir_flag(cli: &Cli, verb: &str) -> AppResult<()> {
    let writes_artifacts =
        release_action(&cli.command).is_some_and(ReleaseAction::writes_artifacts);
    if cli.out_dir.is_some() && !writes_artifacts {
        return Err(only_applies(
            "--out-dir",
            "toven release sbom/depgraphs",
            verb,
        ));
    }
    Ok(())
}

/// Reject the per-run bump argv (`--patch`/`--minor`/`--major`/`--set-version`/
/// `--pre`/`--offline`) on anything but the mutating release actions, which are
/// the only place a version cut happens.
fn gate_bump_flags(cli: &Cli, verb: &str, mutating_release: bool) -> AppResult<()> {
    if mutating_release {
        return Ok(());
    }
    for (present, flag) in [
        (!cli.patch.is_empty(), "--patch"),
        (!cli.minor.is_empty(), "--minor"),
        (!cli.major.is_empty(), "--major"),
        (!cli.set_version.is_empty(), "--set-version"),
        (cli.pre.is_some(), "--pre"),
        (cli.offline, "--offline"),
    ] {
        if present {
            return Err(only_applies(flag, "toven release tag/publish", verb));
        }
    }
    Ok(())
}

/// Reject the `init`-only wizard flags (`--force`/`--root`/`--non-interactive`/
/// `--print`) on any other verb.
fn gate_init_flags(cli: &Cli, verb: &str, is_init: bool) -> AppResult<()> {
    if cli.force.is_some() && !is_init {
        return Err(only_applies("--force", "toven init", verb));
    }
    if cli.root.is_some() && !is_init {
        return Err(only_applies("--root", "toven init", verb));
    }
    if cli.non_interactive && !is_init {
        return Err(only_applies("--non-interactive", "toven init", verb));
    }
    if cli.print && !is_init {
        return Err(only_applies("--print", "toven init", verb));
    }
    Ok(())
}

/// Reject the coverage threshold-override flags (`--line`/`--function`/
/// `--region`/`--changed-line`/`--enforcement`) on any verb but `coverage`,
/// which is the only place a coverage gate runs.
fn gate_coverage_flags(cli: &Cli, verb: &str, is_coverage: bool) -> AppResult<()> {
    if is_coverage {
        return Ok(());
    }
    for (present, flag) in [
        (cli.line.is_some(), "--line"),
        (cli.function.is_some(), "--function"),
        (cli.region.is_some(), "--region"),
        (cli.changed_line.is_some(), "--changed-line"),
        (cli.enforcement.is_some(), "--enforcement"),
    ] {
        if present {
            return Err(only_applies(flag, "toven coverage", verb));
        }
    }
    Ok(())
}

/// Reject the selection flags (`--base`/`--merge-base`,
/// `--module`/`--workspace`/`--dependents`/`--dependencies`) on a verb that
/// performs no selection.
fn gate_selection_flags(cli: &Cli, verb: &str) -> AppResult<()> {
    // `--base` is also the release diff baseline for the mutating release
    // actions; `--merge-base` stays restricted to the changed-selection verbs.
    let mutating_release = release_action(&cli.command).is_some_and(ReleaseAction::is_mutating);
    if cli.base.is_some() && !accepts_baseline(&cli.command) && !mutating_release {
        return Err(AppError::invalid_input(
            "flags",
            format!(
                "`--base` only applies to changed-selection verbs (`toven run`/`toven plan`/`toven affected`/`toven <task>`) and `toven release tag/publish`; it has no effect on `toven {verb}`"
            ),
        ));
    }
    if cli.merge_base && !accepts_baseline(&cli.command) {
        return Err(AppError::invalid_input(
            "flags",
            format!(
                "`--merge-base` only applies to changed-selection verbs (`toven run`/`toven plan`/`toven affected`/`toven <task>`); it has no effect on `toven {verb}`"
            ),
        ));
    }
    if !accepts_selection(&cli.command)
        && (!cli.module.is_empty()
            || !cli.workspace.is_empty()
            || cli.with_dependents
            || cli.with_dependencies)
    {
        let flag = if !cli.module.is_empty() {
            "--module"
        } else if !cli.workspace.is_empty() {
            "--workspace"
        } else if cli.with_dependents {
            "--dependents"
        } else {
            "--dependencies"
        };
        return Err(AppError::invalid_input(
            "flags",
            format!(
                "`{flag}` only applies to selection verbs (`toven run`/`toven plan`/`toven affected`/`toven explain`/`toven <task>`); it has no effect on `toven {verb}`"
            ),
        ));
    }
    Ok(())
}

/// Reject the watch flags (`--watch`, `--watch-debounce-ms`) on a verb that runs
/// no APPLY loop, then enforce the shared APPLY-execution flag combination
/// invariants on the pre-token global flags.
fn gate_watch_flags(cli: &Cli, verb: &str) -> AppResult<()> {
    if cli.watch && !accepts_watch(&cli.command) {
        return Err(AppError::invalid_input(
            "flags",
            format!(
                "`--watch` only applies to task-APPLY verbs (`toven run`/`toven <task>`); it has no effect on `toven {verb}`"
            ),
        ));
    }
    gate_apply_flag_combination(
        cli.watch,
        cli.fail_fast,
        cli.timeout.is_some(),
        cli.is_plan_only(),
        cli.watch_debounce_ms.is_some(),
    )
}

/// Reject APPLY-execution flag *combinations* that are meaningless however the
/// flags arrived — as pre-token globals on a reserved verb, or as trailing tokens
/// on a bare task (which land in [`Command::External`] and never touch [`Cli`]).
///
/// `watch`, `fail_fast`, `timeout_present`, `plan_only`, and `debounce_present`
/// are the effective values after merging global and per-task flags, so both
/// dispatch paths enforce the same invariants: every APPLY-execution flag
/// (`--watch`/`--fail-fast`/`--timeout`) drives real unit execution and so cannot
/// combine with a PLAN-only cut (`--dry-run`/`--explain`), which stops before any
/// unit runs; and the debounce knob is only meaningful with `--watch`.
#[allow(clippy::fn_params_excessive_bools)]
pub(crate) fn gate_apply_flag_combination(
    watch: bool,
    fail_fast: bool,
    timeout_present: bool,
    plan_only: bool,
    debounce_present: bool,
) -> AppResult<()> {
    if plan_only
        && let Some(flag) = watch
            .then_some("--watch")
            .or_else(|| fail_fast.then_some("--fail-fast"))
            .or_else(|| timeout_present.then_some("--timeout"))
    {
        return Err(AppError::invalid_input(
            "flags",
            format!(
                "`{flag}` cannot combine with `--dry-run`/`--explain` (it drives APPLY execution, which a PLAN-only cut skips)"
            ),
        ));
    }
    if debounce_present && !watch {
        return Err(AppError::invalid_input(
            "flags",
            "`--watch-debounce-ms` only applies with `--watch`",
        ));
    }
    Ok(())
}

/// Whether `command` is a task-APPLY verb that consumes `--watch`.
///
/// Watch reruns the affected subgraph through APPLY, so — like `--fail-fast` —
/// only the verbs that drive multi-unit APPLY scheduling read it: `plan` stops at
/// PLAN and `release` runs a single linear pipeline.
const fn accepts_watch(command: &Command) -> bool {
    matches!(command, Command::Run { .. } | Command::External(_))
}

/// The release lifecycle action `command` carries, if it is `toven release`.
const fn release_action(command: &Command) -> Option<ReleaseAction> {
    match command {
        Command::Release { action } => Some(*action),
        _ => None,
    }
}

/// Whether `--dry-run` is meaningful for `command`: a PLAN cut for the task
/// verbs and a no-mutation rehearsal for the rehearsable release actions.
const fn accepts_dry_run(command: &Command) -> bool {
    match command {
        Command::Run { .. } | Command::Plan { .. } | Command::External(_) => true,
        Command::Release { action } => action.accepts_dry_run(),
        _ => false,
    }
}

/// Whether `--explain` (task-planning reasoning detail) is meaningful for
/// `command`; only the task-planning verbs surface it.
const fn accepts_explain(command: &Command) -> bool {
    matches!(
        command,
        Command::Run { .. } | Command::Plan { .. } | Command::External(_)
    )
}

/// Whether `command` builds the human run reporter that `-v`/`-q`/`--color`
/// shape: the execution verbs, the mutating release actions, and `coverage`
/// (which runs the coverage task through a human reporter).
const fn accepts_reporter_shaping(command: &Command) -> bool {
    match command {
        Command::Run { .. } | Command::Plan { .. } | Command::External(_) | Command::Coverage => {
            true
        }
        Command::Release { action } => action.is_mutating(),
        _ => false,
    }
}

/// Whether `command` renders a projection whose format `--output` selects: the
/// execution verbs, every release lifecycle action, `coverage`, and the
/// `tasks`/`modules` listings.
const fn accepts_output_format(command: &Command) -> bool {
    matches!(
        command,
        Command::Run { .. }
            | Command::Plan { .. }
            | Command::External(_)
            | Command::Release { .. }
            | Command::Coverage
            | Command::Tasks { .. }
            | Command::Modules
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

/// Whether `command` builds a cache-aware `PlanRequest` and thus consumes
/// `--no-cache`.
///
/// `run`/`plan`/a bare task all run the cache-aware PLAN spine; `release` runs a
/// separate pipeline without the task cache, so the flag is a no-op there.
const fn accepts_cache_mode(command: &Command) -> bool {
    matches!(
        command,
        Command::Run { .. } | Command::Plan { .. } | Command::External(_)
    )
}

/// Whether `command` performs changed selection and thus consumes the baseline
/// flags (`--base`/`--merge-base`).
const fn accepts_baseline(command: &Command) -> bool {
    matches!(
        command,
        Command::Run { .. }
            | Command::Plan { .. }
            | Command::Affected { .. }
            | Command::Coverage
            | Command::External(_)
    )
}

/// Whether `command` resolves an explicit graph selection and thus consumes the
/// selection flags (`--module`/`--workspace`/`--dependents`/`--dependencies`).
const fn accepts_selection(command: &Command) -> bool {
    matches!(
        command,
        Command::Run { .. }
            | Command::Plan { .. }
            | Command::Affected { .. }
            | Command::Explain { .. }
            | Command::Coverage
            | Command::External(_)
    )
}

fn only_applies(flag: &str, owner: &str, verb: &str) -> AppError {
    AppError::invalid_input(
        "flags",
        format!("`{flag}` only applies to `{owner}` (used with `toven {verb}`)"),
    )
}

/// The shared typed error for combining `--refresh` and `--no-cache`.
///
/// Raised from whichever dispatch path first sees both set (the pre-token `gate`
/// for reserved verbs, or `dispatch_task` for the merged global+task flags on a
/// bare task), so the contradiction is rejected identically either way.
pub(crate) fn refresh_no_cache_conflict() -> AppError {
    AppError::invalid_input(
        "flags",
        "`--refresh` and `--no-cache` are mutually exclusive (`--refresh` re-runs but still writes the cache; `--no-cache` neither reads nor writes)",
    )
}

/// The user-facing name of the dispatched verb (for error messages). Release
/// actions include the action so a gating error names the exact subcommand
/// (`release status`) rather than the bare `release`.
fn verb_name(command: &Command) -> String {
    match command {
        Command::Run { .. } => "run".to_string(),
        Command::Plan { .. } => "plan".to_string(),
        Command::Release { action } => format!("release {}", action.as_str()),
        Command::Coverage => "coverage".to_string(),
        Command::Explain { .. } => "explain".to_string(),
        Command::Init => "init".to_string(),
        Command::Affected { .. } => "affected".to_string(),
        Command::Modules => "modules".to_string(),
        Command::Graph => "graph".to_string(),
        Command::Tasks { .. } => "tasks".to_string(),
        Command::Completions { .. } => "completions".to_string(),
        Command::Driver { .. } => "driver".to_string(),
        Command::Federation { .. } => "federation".to_string(),
        Command::Cache { .. } => "cache".to_string(),
        Command::External(tokens) => tokens.first().map_or("<task>", String::as_str).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Cli, Command, ReleaseAction};
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
    fn explain_takes_a_task_positional_and_reuses_the_module_flag() {
        let cli = parse(&["explain", "build", "--module", "rust:app"]).expect("parses");
        match &cli.command {
            Command::Explain { task } => assert_eq!(task, "build"),
            other => panic!("expected explain, got {other:?}"),
        }
        assert_eq!(cli.module, vec!["rust:app".to_string()]);
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
    fn color_flag_defaults_to_none_and_parses_each_choice() {
        use super::ColorWhen;
        // Unset means "no explicit policy"; `color_choice()` defaults it to auto.
        assert_eq!(parse(&["run", "test"]).unwrap().color, None);
        assert_eq!(
            parse(&["run", "test"]).unwrap().color_choice(),
            ColorWhen::Auto
        );
        assert_eq!(
            parse(&["--color", "always", "run", "test"]).unwrap().color,
            Some(ColorWhen::Always)
        );
        assert_eq!(
            parse(&["--color", "never", "run", "test"]).unwrap().color,
            Some(ColorWhen::Never)
        );
        // An unknown policy is a clap parse error, never a silent fallback.
        assert!(parse(&["--color", "sometimes", "run", "test"]).is_err());
    }

    #[test]
    fn color_rejected_on_non_execution_verbs() {
        // `--color` shapes the human reporter only the execution verbs build, so
        // like `-v`/`-q` it is rejected on introspection/maintenance verbs.
        for args in [
            ["--color", "always", "modules"].as_slice(),
            ["--color", "never", "graph"].as_slice(),
            ["--color", "auto", "cache", "path"].as_slice(),
            ["--color", "always", "tasks"].as_slice(),
        ] {
            let cli = parse(args).expect("parses");
            assert!(super::gate(&cli).is_err(), "{args:?}");
        }
    }

    #[test]
    fn color_accepted_on_execution_verbs() {
        for args in [
            ["--color", "always", "run", "test"].as_slice(),
            ["--color", "never", "plan", "test"].as_slice(),
            ["--color", "auto", "release", "publish"].as_slice(),
            ["--color", "always", "test"].as_slice(),
        ] {
            let cli = parse(args).expect("parses");
            assert!(super::gate(&cli).is_ok(), "{args:?}");
        }
    }

    #[test]
    fn color_choice_maps_onto_the_rskit_policy() {
        use super::ColorWhen;
        use rskit_cli::ColorChoice;
        assert!(matches!(
            ColorChoice::from(ColorWhen::Auto),
            ColorChoice::Auto
        ));
        assert!(matches!(
            ColorChoice::from(ColorWhen::Always),
            ColorChoice::Always
        ));
        assert!(matches!(
            ColorChoice::from(ColorWhen::Never),
            ColorChoice::Never
        ));
    }

    #[test]
    fn help_carries_worked_examples_at_the_top_level_and_per_verb() {
        use clap::CommandFactory;
        let mut command = Cli::command();
        let top = command.render_long_help().to_string();
        assert!(
            top.contains("Examples:") && top.contains("toven tasks"),
            "top-level help is missing its examples block: {top}"
        );
        let tasks = command
            .get_subcommands_mut()
            .find(|sub| sub.get_name() == "tasks")
            .expect("tasks subcommand")
            .render_long_help()
            .to_string();
        assert!(
            tasks.contains("Examples:") && tasks.contains("argv template"),
            "tasks help is missing its examples block: {tasks}"
        );
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
        let cli = parse(&["--allow-dirty", "--no-push", "release", "publish"]).expect("parses");
        assert!(super::gate(&cli).is_ok());
    }

    #[test]
    fn release_requires_a_lifecycle_action() {
        assert!(parse(&["release"]).is_err());
    }

    #[test]
    fn release_actions_parse_to_their_variants() {
        for (arg, want) in [
            ("plan", ReleaseAction::Plan),
            ("status", ReleaseAction::Status),
            ("tag", ReleaseAction::Tag),
            ("publish", ReleaseAction::Publish),
            ("readiness", ReleaseAction::Readiness),
            ("sbom", ReleaseAction::Sbom),
            ("depgraphs", ReleaseAction::Depgraphs),
        ] {
            let cli = parse(&["release", arg]).expect("parses");
            match cli.command {
                Command::Release { action } => {
                    assert_eq!(action.as_str(), want.as_str(), "{arg}");
                }
                other => panic!("expected release, got {other:?}"),
            }
        }
    }

    #[test]
    fn release_projections_are_plan_only() {
        assert!(parse(&["release", "plan"]).unwrap().is_plan_only());
        assert!(parse(&["release", "status"]).unwrap().is_plan_only());
        assert!(parse(&["release", "readiness"]).unwrap().is_plan_only());
        assert!(parse(&["release", "sbom"]).unwrap().is_plan_only());
        assert!(parse(&["release", "depgraphs"]).unwrap().is_plan_only());
        assert!(!parse(&["release", "publish"]).unwrap().is_plan_only());
    }

    #[test]
    fn dirty_and_no_push_only_on_mutating_release_actions() {
        for action in ["tag", "publish"] {
            let cli = parse(&["--allow-dirty", "--no-push", "release", action]).expect("parses");
            assert!(super::gate(&cli).is_ok(), "{action}");
        }
        for action in ["plan", "status", "readiness", "sbom", "depgraphs"] {
            let dirty = parse(&["--allow-dirty", "release", action]).expect("parses");
            assert!(super::gate(&dirty).is_err(), "allow-dirty {action}");
            let no_push = parse(&["--no-push", "release", action]).expect("parses");
            assert!(super::gate(&no_push).is_err(), "no-push {action}");
        }
    }

    #[test]
    fn bump_argv_only_on_mutating_release_actions() {
        let flags = [
            vec!["--minor", "rust:core"],
            vec!["--major", "rust:core"],
            vec!["--patch", "rust:core"],
            vec!["--set-version", "rust:core=1.0.0"],
            vec!["--pre", "rc"],
            vec!["--offline"],
        ];
        for flag in &flags {
            for action in ["tag", "publish"] {
                let mut argv = flag.clone();
                argv.extend(["release", action]);
                let cli = parse(&argv).expect("parses");
                assert!(super::gate(&cli).is_ok(), "{flag:?} on {action}");
            }
            for action in ["plan", "status", "readiness", "sbom", "depgraphs"] {
                let mut argv = flag.clone();
                argv.extend(["release", action]);
                let cli = parse(&argv).expect("parses");
                assert!(super::gate(&cli).is_err(), "{flag:?} on {action}");
            }
        }
    }

    #[test]
    fn out_dir_only_on_artifact_release_actions() {
        for action in ["sbom", "depgraphs"] {
            let cli = parse(&["--out-dir", "/tmp/out", "release", action]).expect("parses");
            assert!(super::gate(&cli).is_ok(), "{action}");
        }
        for action in ["plan", "status", "readiness", "tag", "publish"] {
            let cli = parse(&["--out-dir", "/tmp/out", "release", action]).expect("parses");
            assert!(super::gate(&cli).is_err(), "{action}");
        }
    }

    #[test]
    fn base_accepted_on_mutating_release_but_merge_base_is_not() {
        for action in ["tag", "publish"] {
            let cli = parse(&["--base", "v1.0.0", "release", action]).expect("parses");
            assert!(super::gate(&cli).is_ok(), "base {action}");
            let merge = parse(&["--merge-base", "release", action]).expect("parses");
            assert!(super::gate(&merge).is_err(), "merge-base {action}");
        }
        for action in ["plan", "status"] {
            let cli = parse(&["--base", "v1.0.0", "release", action]).expect("parses");
            assert!(super::gate(&cli).is_err(), "base {action}");
        }
    }

    #[test]
    fn dry_run_only_on_rehearsable_release_actions() {
        for action in ["plan", "publish"] {
            let cli = parse(&["--dry-run", "release", action]).expect("parses");
            assert!(super::gate(&cli).is_ok(), "{action}");
        }
        for action in ["status", "tag", "readiness", "sbom", "depgraphs"] {
            let cli = parse(&["--dry-run", "release", action]).expect("parses");
            assert!(super::gate(&cli).is_err(), "{action}");
        }
    }

    #[test]
    fn explain_rejected_on_every_release_action() {
        for action in [
            "plan",
            "status",
            "tag",
            "publish",
            "readiness",
            "sbom",
            "depgraphs",
        ] {
            let cli = parse(&["--explain", "release", action]).expect("parses");
            assert!(super::gate(&cli).is_err(), "{action}");
        }
    }

    #[test]
    fn reporter_shaping_only_on_mutating_release_actions() {
        for action in ["tag", "publish"] {
            let cli = parse(&["--verbose", "release", action]).expect("parses");
            assert!(super::gate(&cli).is_ok(), "{action}");
            let colored = parse(&["--color", "auto", "release", action]).expect("parses");
            assert!(super::gate(&colored).is_ok(), "color {action}");
        }
        for action in ["plan", "status", "readiness", "sbom", "depgraphs"] {
            let cli = parse(&["--verbose", "release", action]).expect("parses");
            assert!(super::gate(&cli).is_err(), "{action}");
            let colored = parse(&["--color", "auto", "release", action]).expect("parses");
            assert!(super::gate(&colored).is_err(), "color {action}");
        }
    }

    #[test]
    fn output_format_accepted_on_every_release_action() {
        for action in [
            "plan",
            "status",
            "tag",
            "publish",
            "readiness",
            "sbom",
            "depgraphs",
        ] {
            let cli = parse(&["--output", "jsonl", "release", action]).expect("parses");
            assert!(super::gate(&cli).is_ok(), "{action}");
        }
    }

    #[test]
    fn release_action_classification() {
        assert!(ReleaseAction::Plan.is_projection());
        assert!(ReleaseAction::Status.is_projection());
        assert!(ReleaseAction::Readiness.is_projection());
        assert!(ReleaseAction::Sbom.is_projection());
        assert!(ReleaseAction::Depgraphs.is_projection());
        assert!(!ReleaseAction::Tag.is_projection());
        assert!(ReleaseAction::Tag.is_mutating());
        assert!(ReleaseAction::Publish.is_mutating());
        assert!(!ReleaseAction::Plan.is_mutating());
        assert!(!ReleaseAction::Readiness.is_mutating());
        assert!(ReleaseAction::Sbom.writes_artifacts());
        assert!(ReleaseAction::Depgraphs.writes_artifacts());
        assert!(!ReleaseAction::Readiness.writes_artifacts());
        assert!(ReleaseAction::Plan.accepts_dry_run());
        assert!(ReleaseAction::Publish.accepts_dry_run());
        assert!(!ReleaseAction::Status.accepts_dry_run());
        assert!(!ReleaseAction::Tag.accepts_dry_run());
        assert!(!ReleaseAction::Readiness.accepts_dry_run());
    }

    #[test]
    fn init_flags_only_on_init() {
        for args in [
            ["--non-interactive", "init"].as_slice(),
            ["--print", "init"].as_slice(),
            ["--root", "/tmp", "init"].as_slice(),
            ["--force", "rust", "init"].as_slice(),
        ] {
            assert!(super::gate(&parse(args).unwrap()).is_ok(), "{args:?}");
        }
        for args in [
            ["--non-interactive", "plan", "test"].as_slice(),
            ["--print", "plan", "test"].as_slice(),
            ["--root", "/tmp", "modules"].as_slice(),
            ["--force", "rust", "graph"].as_slice(),
        ] {
            assert!(super::gate(&parse(args).unwrap()).is_err(), "{args:?}");
        }
    }

    #[test]
    fn init_accepts_the_yes_alias() {
        let cli = parse(&["--yes", "init"]).expect("parses");
        assert!(cli.non_interactive);
        assert!(super::gate(&cli).is_ok());
    }

    #[test]
    fn graph_format_only_on_graph() {
        assert!(super::gate(&parse(&["--format", "dot", "graph"]).unwrap()).is_ok());
        assert!(super::gate(&parse(&["--format", "dot", "plan", "t"]).unwrap()).is_err());
    }

    #[test]
    fn no_cache_only_on_task_verbs() {
        for args in [
            ["--no-cache", "run", "test"].as_slice(),
            ["--no-cache", "plan", "test"].as_slice(),
        ] {
            assert!(super::gate(&parse(args).unwrap()).is_ok(), "{args:?}");
        }
        for args in [
            ["--no-cache", "release", "publish"].as_slice(),
            ["--no-cache", "affected", "test"].as_slice(),
            ["--no-cache", "modules"].as_slice(),
        ] {
            assert!(super::gate(&parse(args).unwrap()).is_err(), "{args:?}");
        }
    }

    #[test]
    fn execution_flags_rejected_on_cache() {
        assert!(super::gate(&parse(&["--dry-run", "cache", "path"]).unwrap()).is_err());
    }

    #[test]
    fn refresh_only_on_cache_aware_verbs() {
        for args in [
            ["--refresh", "run", "test"].as_slice(),
            ["--refresh", "plan", "test"].as_slice(),
        ] {
            assert!(super::gate(&parse(args).unwrap()).is_ok(), "{args:?}");
        }
        for args in [
            ["--refresh", "release", "publish"].as_slice(),
            ["--refresh", "affected", "test"].as_slice(),
            ["--refresh", "modules"].as_slice(),
        ] {
            assert!(super::gate(&parse(args).unwrap()).is_err(), "{args:?}");
        }
    }

    #[test]
    fn refresh_and_no_cache_are_mutually_exclusive() {
        assert!(super::gate(&parse(&["--refresh", "--no-cache", "run", "test"]).unwrap()).is_err());
    }

    #[test]
    fn timeout_only_on_task_apply_verbs() {
        for args in [
            ["--timeout", "5s", "run", "test"].as_slice(),
            ["--timeout", "2m", "test"].as_slice(),
        ] {
            assert!(super::gate(&parse(args).unwrap()).is_ok(), "{args:?}");
        }
        // `plan` stops at PLAN and `release`/introspection never run bounded
        // units, so the bound is a no-op and is rejected.
        for args in [
            ["--timeout", "5s", "plan", "test"].as_slice(),
            ["--timeout", "5s", "release", "publish"].as_slice(),
            ["--timeout", "5s", "affected", "test"].as_slice(),
        ] {
            assert!(super::gate(&parse(args).unwrap()).is_err(), "{args:?}");
        }
    }

    #[test]
    fn timeout_rejects_a_malformed_or_zero_duration() {
        assert!(parse(&["--timeout", "soon", "test"]).is_err());
        assert!(parse(&["--timeout", "0s", "test"]).is_err());
    }

    #[test]
    fn watch_only_on_task_apply_verbs() {
        for args in [
            ["--watch", "run", "test"].as_slice(),
            ["--watch", "test"].as_slice(),
        ] {
            assert!(super::gate(&parse(args).unwrap()).is_ok(), "{args:?}");
        }
        for args in [
            ["--watch", "plan", "test"].as_slice(),
            ["--watch", "affected", "test"].as_slice(),
            ["--watch", "release", "publish"].as_slice(),
            ["--watch", "graph"].as_slice(),
        ] {
            assert!(super::gate(&parse(args).unwrap()).is_err(), "{args:?}");
        }
    }

    #[test]
    fn watch_rejects_plan_only_cuts() {
        assert!(super::gate(&parse(&["--watch", "--dry-run", "run", "test"]).unwrap()).is_err());
        assert!(super::gate(&parse(&["--watch", "--explain", "run", "test"]).unwrap()).is_err());
    }

    #[test]
    fn apply_execution_flags_reject_plan_only_cuts() {
        // `--fail-fast`/`--timeout`, like `--watch`, drive real unit execution,
        // so combining them with a PLAN-only cut (`--dry-run`/`--explain`) — even
        // on an APPLY verb that otherwise accepts them — is rejected rather than
        // silently ignored.
        for args in [
            ["--fail-fast", "--dry-run", "run", "test"].as_slice(),
            ["--fail-fast", "--explain", "test"].as_slice(),
            ["--timeout", "5s", "--dry-run", "run", "test"].as_slice(),
            ["--timeout", "5s", "--explain", "test"].as_slice(),
        ] {
            assert!(super::gate(&parse(args).unwrap()).is_err(), "{args:?}");
        }
    }

    #[test]
    fn apply_flag_combination_rejects_each_execution_flag_under_plan_only() {
        assert!(super::gate_apply_flag_combination(true, false, false, true, false).is_err());
        assert!(super::gate_apply_flag_combination(false, true, false, true, false).is_err());
        assert!(super::gate_apply_flag_combination(false, false, true, true, false).is_err());
        // No PLAN-only cut: every execution flag is accepted.
        assert!(super::gate_apply_flag_combination(true, true, true, false, false).is_ok());
    }

    #[test]
    fn watch_debounce_requires_watch() {
        assert!(
            super::gate(&parse(&["--watch-debounce-ms", "500", "run", "test"]).unwrap()).is_err()
        );
        assert!(
            super::gate(&parse(&["--watch", "--watch-debounce-ms", "500", "run", "test"]).unwrap())
                .is_ok()
        );
    }

    #[test]
    fn execution_flags_rejected_on_introspection_verbs() {
        for args in [
            ["--dry-run", "explain", "test"].as_slice(),
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
            ["--output", "jsonl", "release", "plan"].as_slice(),
            ["--fail-fast", "test"].as_slice(),
        ] {
            let cli = parse(args).expect("parses");
            assert!(super::gate(&cli).is_ok(), "{args:?}");
        }
    }

    #[test]
    fn output_format_accepted_on_the_modules_projection() {
        let cli = parse(["--output", "jsonl", "modules"].as_slice()).expect("parses");
        assert!(super::gate(&cli).is_ok());
    }

    #[test]
    fn fail_fast_rejected_on_non_apply_verbs() {
        // `--fail-fast` shapes APPLY scheduling, so it is a no-op on `plan`
        // (PLAN-only) and `release` (single linear pipeline) and rejected there.
        for args in [
            ["--fail-fast", "plan", "test"].as_slice(),
            ["--fail-fast", "release", "publish"].as_slice(),
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
            ["--quiet", "explain", "test"].as_slice(),
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
            ["--verbose", "release", "publish"].as_slice(),
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
            ["--merge-base", "release", "publish"].as_slice(),
            ["--merge-base", "modules"].as_slice(),
            ["--base", "origin/main", "graph"].as_slice(),
            ["--merge-base", "explain", "test"].as_slice(),
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
