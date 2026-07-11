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
    /// One live, fixed-height tile per in-flight unit in a single terminal.
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
    /// Plan and publish a release.
    Release,
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
    let verb = verb_name(&cli.command);
    let is_release = matches!(cli.command, Command::Release);
    let is_init = matches!(cli.command, Command::Init);
    let is_graph = matches!(cli.command, Command::Graph);

    if cli.allow_dirty && !is_release {
        return Err(only_applies("--allow-dirty", "toven release", verb));
    }
    if cli.no_push && !is_release {
        return Err(only_applies("--no-push", "toven release", verb));
    }
    gate_init_flags(cli, verb, is_init)?;
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
    if (cli.dry_run || cli.explain) && !accepts_execution_flags(&cli.command) {
        return Err(AppError::invalid_input(
            "flags",
            format!("execution flags (--dry-run/--explain) do not apply to `toven {verb}`"),
        ));
    }
    // `--output` selects the event-sink/projection format; the execution verbs
    // and the `tasks` discovery verb render a chooseable projection, but the
    // other introspection/maintenance verbs print their own fixed rendering.
    if cli.output.is_some() && !accepts_output_format(&cli.command) {
        return Err(AppError::invalid_input(
            "flags",
            format!("`--output` does not apply to `toven {verb}`"),
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
    // `--color` shapes the same human reporter as `-v`/`-q`; only the execution
    // verbs build it. An explicit `--color` on an introspection/maintenance verb
    // would be a silent no-op, so reject it rather than advertise one.
    if cli.color.is_some() && !accepts_execution_flags(&cli.command) {
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
    if cli.fail_fast && !accepts_fail_fast(&cli.command) {
        return Err(AppError::invalid_input(
            "flags",
            format!(
                "`--fail-fast` only applies to task-APPLY verbs (`toven run`/`toven <task>`); it has no effect on `toven {verb}`"
            ),
        ));
    }
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
    // `--timeout` bounds APPLY execution, so — like `--fail-fast`/`--watch` — it
    // is meaningful only on the task-APPLY verbs that actually run units.
    if cli.timeout.is_some() && !accepts_fail_fast(&cli.command) {
        return Err(AppError::invalid_input(
            "flags",
            format!(
                "`--timeout` only applies to task-APPLY verbs (`toven run`/`toven <task>`); it has no effect on `toven {verb}`"
            ),
        ));
    }
    gate_watch_flags(cli, verb)?;
    // `--base`/`--merge-base` only shape changed selection, and
    // `--module`/`--workspace`/`--with-dependents` shape explicit selection —
    // both belong to the same selection verbs; other verbs would ignore them.
    gate_selection_flags(cli, verb)?;
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

/// Reject the selection flags (`--base`/`--merge-base`,
/// `--module`/`--workspace`/`--dependents`/`--dependencies`) on a verb that
/// performs no selection.
fn gate_selection_flags(cli: &Cli, verb: &str) -> AppResult<()> {
    if !accepts_baseline(&cli.command) && (cli.base.is_some() || cli.merge_base) {
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

/// Whether `command` is an execution verb that accepts reporter-shaping flags
/// (`--dry-run`/`--explain` and the `-v`/`-q` verbosity counts).
const fn accepts_execution_flags(command: &Command) -> bool {
    matches!(
        command,
        Command::Run { .. } | Command::Plan { .. } | Command::Release | Command::External(_)
    )
}

/// Whether `command` renders a projection whose format `--output` selects: the
/// execution verbs, the `tasks` discovery verb, and the `modules` listing.
const fn accepts_output_format(command: &Command) -> bool {
    accepts_execution_flags(command) || matches!(command, Command::Tasks { .. } | Command::Modules)
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

/// The user-facing name of the dispatched verb (for error messages).
fn verb_name(command: &Command) -> &str {
    match command {
        Command::Run { .. } => "run",
        Command::Plan { .. } => "plan",
        Command::Release => "release",
        Command::Explain { .. } => "explain",
        Command::Init => "init",
        Command::Affected { .. } => "affected",
        Command::Modules => "modules",
        Command::Graph => "graph",
        Command::Tasks { .. } => "tasks",
        Command::Completions { .. } => "completions",
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
            ["--color", "auto", "release"].as_slice(),
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
        let cli = parse(&["--allow-dirty", "--no-push", "release"]).expect("parses");
        assert!(super::gate(&cli).is_ok());
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
            ["--no-cache", "release"].as_slice(),
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
            ["--refresh", "release"].as_slice(),
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
            ["--timeout", "5s", "release"].as_slice(),
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
            ["--watch", "release"].as_slice(),
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
            ["--output", "jsonl", "release"].as_slice(),
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
