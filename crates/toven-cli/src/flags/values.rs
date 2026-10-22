//! Flag value types and the shared clap value parsers.

use std::time::Duration;

use clap::ValueEnum;
use rskit_util::time::parse_duration;
use toven_ports::ComputeBudget;

/// Default trailing-edge debounce window (ms) for `--watch` when
/// `--watch-debounce-ms` is not given.
pub const DEFAULT_WATCH_DEBOUNCE_MS: u64 = 200;

/// Parse a `--timeout` duration string (e.g. `30s`, `5m`) into a [`Duration`].
///
/// Rejects a zero or
/// unparseable duration — a bound of zero would fail every unit immediately,
/// which is never what the user means.
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

/// Parse `--compute-budget`: `auto`, `inherit`, or a non-negative integer
/// (`0` = `inherit`).
///
/// Mirrors the `[toven].compute_budget` config grammar so the flag and the
/// config setting accept the same spellings.
pub(crate) fn parse_compute_budget_arg(value: &str) -> Result<ComputeBudget, String> {
    match value {
        "auto" => Ok(ComputeBudget::Auto),
        "inherit" => Ok(ComputeBudget::Inherit),
        other => match other.parse::<usize>() {
            Ok(threads) => Ok(ComputeBudget::fixed(threads)),
            Err(error) => Err(format!(
                "`--compute-budget` requires `auto`, `inherit`, or a non-negative integer (got `{value}`): {error}"
            )),
        },
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

/// Output format selected by `--output`: human tables or machine-readable JSONL.
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
/// Mirrors the engine's [`ViewMode`](toven_core::config::ViewMode) so a flag
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
    /// One multiplexer pane per unit (opt-in; requires a supported
    /// multiplexer).
    Panes,
    /// A single linear stream, log-friendly (each unit flushed as one block).
    Stream,
}

impl From<ViewMode> for toven_core::config::ViewMode {
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
/// Maps to rskit's [`ColorChoice`](rskit_cli::ColorChoice); the `NO_COLOR` environment variable always overrides an explicit `always`, following the [`NO_COLOR` standard](https://no-color.org).
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
/// Derived from the net of the repeatable `--verbose` and `--quiet` counts:
/// each `-v` raises the level and each `-q` lowers it. The level selects how
/// much of the engine's [`Event`](toven_model::Event) stream the human reporter
/// renders; the machine-parseable JSON-lines stream is unaffected (it always
/// carries every event so consumers see the full record).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Verbosity {
    /// Only the run-level lines (start + the terminal summary).
    Quiet,
    /// The default: run, plan, and terminal per-unit results.
    #[default]
    Normal,
    /// Everything, including per-phase, cache-decision, and unit-lifecycle
    /// lines.
    Verbose,
}

impl Verbosity {
    /// Resolve the level from the net of the `verbose` and `quiet` repeat
    /// counts.
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
