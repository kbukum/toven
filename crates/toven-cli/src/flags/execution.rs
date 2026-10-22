//! The execution flags shared by every task verb and a bare task.

use std::path::PathBuf;
use std::time::Duration;

use clap::Args;
use rskit_errors::{AppError, AppResult};
use toven_core::vcs::BaselineFlags;
use toven_ports::ComputeBudget;

use super::values::{
    ColorWhen, OutputKind, Verbosity, ViewMode, parse_compute_budget_arg, parse_duration_arg,
    parse_jobs_arg,
};

/// The global flags a bare task also accepts after its name.
///
/// [`Cli`](super::Cli) flattens this group, so the flags parse before or after
/// any verb; [`grammar`](crate::grammar) parses a bare task's flag prefix with
/// the same group, then [`merged_with`](Self::merged_with) combines the two.
#[derive(Debug, Clone, Default, PartialEq, Eq, Args)]
#[allow(clippy::struct_excessive_bools)]
pub struct ExecutionArgs {
    /// Path to the `toven.toml` config (otherwise discovered upward from the
    /// cwd).
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,
    /// Output format: `human` tables or machine-readable `jsonl` (defaults to the
    /// `[toven].report` setting).
    #[arg(long, global = true, value_name = "FORMAT")]
    pub output: Option<OutputKind>,
    /// When to colorize human output: `auto` (default), `always`, or `never`.
    /// `NO_COLOR` always wins over `always`. Accepted on every command; commands
    /// that produce no colorized output simply ignore it.
    #[arg(long, global = true, value_name = "WHEN")]
    pub color: Option<ColorWhen>,
    /// How live per-unit output renders while a task runs: `auto` (default),
    /// `tiles`, `panes`, or `stream`. Applies only when running tasks
    /// (`toven run` / `toven <task>`); overrides the `[toven].view` setting.
    /// Redirected, piped, `--output jsonl`, and non-interactive runs always use
    /// the linear `stream` shape regardless of this flag.
    #[arg(long, global = true, value_name = "MODE", help_heading = "Execution")]
    pub view: Option<ViewMode>,
    /// Preview what would run, without executing anything.
    #[arg(long, global = true, help_heading = "Execution")]
    pub dry_run: bool,
    /// Preview what would run, with the reasoning for each unit.
    #[arg(long, global = true, help_heading = "Execution")]
    pub explain: bool,
    /// Stop scheduling after the first failure (when running tasks only).
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
    /// When running tasks only: bound how long any single execution unit may run
    /// before it is cooperatively cancelled and reported as a timeout failure
    /// (duration string, e.g. `30s`, `5m`).
    #[arg(long, global = true, value_name = "DURATION", value_parser = parse_duration_arg, help_heading = "Execution")]
    pub timeout: Option<Duration>,
    /// When running tasks only: cap how many units run concurrently, overriding
    /// the `[toven].max_parallel` setting. `--jobs 1` forces strictly serial
    /// execution (one unit at a time), which streams each unit's output inline
    /// as a single continuous log instead of buffered per-unit blocks.
    #[arg(long, short = 'j', global = true, value_name = "N", value_parser = parse_jobs_arg, help_heading = "Execution")]
    pub jobs: Option<usize>,
    /// When running tasks only: cap the CPU parallelism handed to each spawned
    /// tool, overriding `[toven].compute_budget`. The engine splits this total
    /// budget across the units running concurrently and injects each share as an
    /// ecosystem env var (e.g. `GOMAXPROCS`). `auto` sizes it to the host CPUs;
    /// `inherit` (or `0`) injects nothing.
    #[arg(long, global = true, value_name = "BUDGET", value_parser = parse_compute_budget_arg, help_heading = "Execution")]
    pub compute_budget: Option<ComputeBudget>,
    /// Change-selection commands only: override the diff baseline reference
    /// (per-member under a federation; falls back to `[[members]].base_ref` /
    /// `[project].base_ref`).
    #[arg(long, global = true, value_name = "REF", help_heading = "Selection")]
    pub base: Option<String>,
    /// Change-selection commands only: diff against `merge-base(reference,
    /// HEAD)`.
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
    /// `--workspace`, also activate the forward-dependencies closure
    /// (everything the selection needs).
    #[arg(long = "dependencies", global = true, help_heading = "Selection")]
    pub with_dependencies: bool,
    /// When running tasks only: keep running, re-executing the affected subgraph
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
}

impl ExecutionArgs {
    /// Merge a bare task's own flags into the globals, then check the merged
    /// set. `gate` defers the combination check for bare tasks to here,
    /// because only the merged set is meaningful.
    ///
    /// # Errors
    /// Returns a typed usage error naming the conflicting flag.
    pub fn for_task(self, task: Self) -> AppResult<Self> {
        let merged = self.merged_with(task);
        merged.check_combinations(merged.is_plan_only())?;
        Ok(merged)
    }

    /// Combine pre-token globals (`self`) with a bare task's own flags
    /// (`task`): a task value wins, switches and counts add up, and repeatable
    /// selectors accumulate.
    #[must_use]
    pub fn merged_with(self, task: Self) -> Self {
        let mut module = self.module;
        module.extend(task.module);
        let mut workspace = self.workspace;
        workspace.extend(task.workspace);
        Self {
            config: task.config.or(self.config),
            output: task.output.or(self.output),
            color: task.color.or(self.color),
            view: task.view.or(self.view),
            dry_run: self.dry_run || task.dry_run,
            explain: self.explain || task.explain,
            fail_fast: self.fail_fast || task.fail_fast,
            no_cache: self.no_cache || task.no_cache,
            refresh: self.refresh || task.refresh,
            timeout: task.timeout.or(self.timeout),
            jobs: task.jobs.or(self.jobs),
            compute_budget: task.compute_budget.or(self.compute_budget),
            base: task.base.or(self.base),
            merge_base: self.merge_base || task.merge_base,
            module,
            workspace,
            with_dependents: self.with_dependents || task.with_dependents,
            with_dependencies: self.with_dependencies || task.with_dependencies,
            watch: self.watch || task.watch,
            watch_debounce_ms: task.watch_debounce_ms.or(self.watch_debounce_ms),
            verbose: self.verbose.saturating_add(task.verbose),
            quiet: self.quiet.saturating_add(task.quiet),
        }
    }

    /// Whether `--dry-run` or `--explain` asks for a PLAN-only cut.
    #[must_use]
    pub const fn is_plan_only(&self) -> bool {
        self.dry_run || self.explain
    }

    /// Reject flag combinations that contradict each other, whichever route
    /// they arrived by.
    ///
    /// `--refresh` and `--no-cache` are opposite cache policies. Every
    /// APPLY-execution flag (`--watch`/`--fail-fast`/`--timeout`) drives real
    /// unit execution, so it cannot combine with a PLAN-only cut (`plan_only`),
    /// which stops before any unit runs. The debounce knob only applies with
    /// `--watch`.
    ///
    /// # Errors
    /// Returns a typed usage error naming the conflicting flag.
    pub fn check_combinations(&self, plan_only: bool) -> AppResult<()> {
        if self.refresh && self.no_cache {
            return Err(refresh_no_cache_conflict());
        }
        if plan_only
            && let Some(flag) = self
                .watch
                .then_some("--watch")
                .or_else(|| self.fail_fast.then_some("--fail-fast"))
                .or_else(|| self.timeout.is_some().then_some("--timeout"))
        {
            return Err(AppError::invalid_input(
                "flags",
                format!(
                    "`{flag}` cannot combine with `--dry-run`/`--explain` (it drives APPLY execution, which a PLAN-only cut skips)"
                ),
            ));
        }
        if self.watch_debounce_ms.is_some() && !self.watch {
            return Err(AppError::invalid_input(
                "flags",
                "`--watch-debounce-ms` only applies with `--watch`",
            ));
        }
        Ok(())
    }

    /// The reporter verbosity selected by the `-v`/`-q` counts.
    #[must_use]
    pub fn verbosity(&self) -> Verbosity {
        Verbosity::for_execution(self.verbose, self.quiet, self.explain)
    }

    /// The color policy applied to the human reporter, defaulting to `auto`
    /// when `--color` is not given.
    #[must_use]
    pub fn color_choice(&self) -> ColorWhen {
        self.color.unwrap_or(ColorWhen::Auto)
    }

    /// The baseline selection (`--base`/`--merge-base`) threaded into
    /// changed-selection. Empty when neither flag is given, so the engine
    /// falls back to the configured `base_ref`.
    #[must_use]
    pub fn baseline_flags(&self) -> BaselineFlags {
        let mut flags = BaselineFlags::new().with_merge_base(self.merge_base);
        if let Some(reference) = &self.base {
            flags = flags.with_base(reference.clone());
        }
        flags
    }
}

/// The shared typed error for combining `--refresh` and `--no-cache`.
fn refresh_no_cache_conflict() -> AppError {
    AppError::invalid_input(
        "flags",
        "`--refresh` and `--no-cache` are mutually exclusive (`--refresh` re-runs but still writes the cache; `--no-cache` neither reads nor writes)",
    )
}
