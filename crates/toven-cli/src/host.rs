//! Shared wiring for the command verbs: config discovery, project load, the
//! injected engine host, and reporter/cache-root resolution.
//!
//! Every verb needs the same preamble — locate `toven.toml`, load the strict
//! [`Document`], resolve the workspace root, and bind the rskit-backed git /
//! digest / probe / cache ports the engine injects. This module owns that
//! preamble so the verb modules stay focused on their projection or execution.
//! It is wiring only: it prints nothing (the reporter sinks do) and returns
//! typed data + typed errors.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_util::time::{Clock, FixedClock, SharedClock, system_clock};
use toven_engine::cache;
use toven_engine_core::config::{CanonicalRegistry, Document, ReportFormat, load};
use toven_engine_core::federation::{OpenMemberVcsReaders, open_project_vcs};
use toven_engine_core::vcs::BaselineFlags;
use toven_model::AbsPath;
use toven_ports::Provider;

use crate::flags::{ColorWhen, OutputKind, Verbosity};
use crate::report::{HumanReporter, JsonlReporter};

/// The canonical `toven.toml` config filename.
const CONFIG_FILENAME: &str = "toven.toml";

/// A loaded project: the strict document plus the resolved workspace root.
pub(crate) struct Project {
    /// The strict, structurally-validated configuration document.
    pub(crate) document: Document,
    /// Absolute workspace root (`config_dir` joined with `[project].root`).
    pub(crate) project_root: AbsPath,
}

impl Project {
    /// The configured cache-root override (`[toven.cache].dir`), if any.
    fn cache_dir(&self) -> Option<&str> {
        self.document.toven.cache.dir.as_deref()
    }

    /// Resolve the on-disk cache root for this project.
    ///
    /// # Errors
    /// Propagates cache-root resolution failures (traversal / platform dir).
    pub(crate) fn cache_root(&self) -> AppResult<PathBuf> {
        cache::resolve_root(&self.project_root, self.cache_dir())
    }

    /// Open one deduped git reader/writer per composed member repo.
    ///
    /// Both PLAN change selection and release borrow this opened set; the
    /// single-repo project is the N=1 degenerate member at the umbrella root.
    ///
    /// # Errors
    /// Propagates member composition and repository discovery/open failures.
    pub(crate) fn open_member_vcs(
        &self,
        providers: &[&dyn Provider],
        flags: &BaselineFlags,
    ) -> AppResult<OpenMemberVcsReaders> {
        open_project_vcs(&self.project_root, &self.document, providers, flags)
    }

    /// The configured concurrency ceiling, if `[toven].max_parallel` is set.
    #[must_use]
    pub(crate) const fn max_parallel(&self) -> Option<usize> {
        self.document.toven.max_parallel
    }
}

/// Locate the config file: the explicit `--config` path, else the nearest
/// `toven.toml` walking up from the current directory.
///
/// # Errors
/// Returns a not-found error when no config is given and none is discovered.
pub(crate) fn discover_config(explicit: Option<&Path>) -> AppResult<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path.to_path_buf());
    }
    let cwd = std::env::current_dir().map_err(AppError::internal)?;
    rskit_fs::find_in_ancestors(&cwd, CONFIG_FILENAME).ok_or_else(|| {
        // One clean sentence plus the recovery step. `AppError::not_found`'s
        // `resource '<id>' not found` template would double-wrap this (the id
        // slot is for a value, not a sentence), so build the message directly
        // under the NotFound code — the renderer prefixes `error[NOT_FOUND]:`
        // and the exit code stays 4.
        AppError::new(
            ErrorCode::NotFound,
            "no toven.toml found in this directory or any parent — run `toven init` to create one",
        )
    })
}

/// Load and validate the project at `config_path` against the compiled-in
/// `providers`.
///
/// # Errors
/// Propagates config parse/validation/dispatch failures and workspace-root
/// resolution failures.
pub(crate) fn load_project(config_path: &Path, providers: &[&dyn Provider]) -> AppResult<Project> {
    let loaded_ids = providers
        .iter()
        .map(|provider| provider.ecosystem_id().clone())
        .collect();
    let canonical = CanonicalRegistry::model();
    let loaded = load(config_path, &loaded_ids, &canonical)?;

    let config_dir = absolute(config_path)?
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| AppError::invalid_input("config", "config path has no parent directory"))?;
    let root = config_dir.join(&loaded.document.project.root);
    let project_root = AbsPath::new(absolute(&root)?)?;

    Ok(Project {
        document: loaded.document,
        project_root,
    })
}

/// Canonicalize `path` to an absolute path, surfacing IO failures as typed
/// errors.
///
/// Delegates to rskit-fs so canonicalization follows the canonical filesystem
/// error/cause policy rather than re-deriving it from `std::fs` here.
fn absolute(path: &Path) -> AppResult<PathBuf> {
    rskit_fs::canonicalize(path)
}

/// The resolved event-sink format for a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    /// Human-readable terminal rendering.
    Human,
    /// Machine-parseable JSON-lines.
    Jsonl,
}

/// The resolved reporter binding for a run: the sink format plus the verbosity
/// that the human sink renders at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Report {
    format: Format,
    verbosity: Verbosity,
    color: ColorWhen,
}

impl Report {
    /// Resolve the reporter binding: the `--output` flag wins for the format,
    /// else the `[toven].report` document setting; `verbosity` is the resolved
    /// `-v`/`-q` level; `color` is the `--color` policy applied to the human
    /// sink (the machine projection is never colorized).
    #[must_use]
    pub(crate) const fn resolve(
        flag: Option<OutputKind>,
        verbosity: Verbosity,
        color: ColorWhen,
        document: &Document,
    ) -> Self {
        let format = match flag {
            Some(OutputKind::Human) => Format::Human,
            Some(OutputKind::Jsonl) => Format::Jsonl,
            None => match document.toven.report {
                ReportFormat::Json => Format::Jsonl,
                _ => Format::Human,
            },
        };
        Self {
            format,
            verbosity,
            color,
        }
    }

    /// Build the matching reporter sink at the resolved verbosity.
    ///
    /// The human sink lands on stderr (its progress/status/summary lines are
    /// diagnostics), while the Jsonl sink lands on stdout as the
    /// machine-readable projection. The verbosity filters the human reporter's
    /// rendering of the Event stream; the JSON-lines sink ignores it and always
    /// emits every event so a machine consumer sees the complete record. The
    /// `--color` policy is resolved against stderr's terminal state and applied
    /// to the human sink only — the machine projection stays byte-stable.
    #[must_use]
    pub(crate) fn reporter(self) -> Box<dyn toven_ports::Reporter> {
        match self.format {
            Format::Human => {
                let palette = rskit_cli::Palette::for_stream(self.color.into(), &std::io::stderr());
                Box::new(HumanReporter::stderr(self.verbosity).with_palette(palette))
            }
            Format::Jsonl => Box::new(JsonlReporter::stdout()),
        }
    }

    /// Whether the machine-readable JSON-lines projection is active, in which
    /// case live child output must keep the byte-stable `stream` shape (never a
    /// tiles/panes live area) so a piped consumer is unaffected.
    #[must_use]
    pub(crate) const fn forces_stream_output(self) -> bool {
        matches!(self.format, Format::Jsonl)
    }

    /// The stderr palette for the live raw-output renderer, folding `--color`
    /// with stderr's terminal state exactly as the human reporter does.
    #[must_use]
    pub(crate) fn stderr_palette(self) -> rskit_cli::Palette {
        rskit_cli::Palette::for_stream(self.color.into(), &std::io::stderr())
    }
}

/// Resolve the effective projection format for an introspection verb: the
/// explicit `--output` flag wins, else the `[toven].report` document setting,
/// so a discovery verb honors a config-driven default the same way the run
/// reporter does via [`Report::resolve`].
#[must_use]
pub(crate) const fn resolve_output(flag: Option<OutputKind>, document: &Document) -> OutputKind {
    match flag {
        Some(kind) => kind,
        None => match document.toven.report {
            ReportFormat::Json => OutputKind::Jsonl,
            _ => OutputKind::Human,
        },
    }
}

/// Environment variable that pins the CLI wall clock to a fixed Unix epoch
/// second.
///
/// Unset in normal use, so [`new_run_id`] reads the real system clock. When set
/// to a `u64`, the clock is replaced by a [`FixedClock`] at that epoch, which
/// makes the emitted `run_id` — and therefore the whole machine-readable Event
/// stream — deterministic. End-to-end/snapshot harnesses set this so they can
/// match the `jsonl` projection byte-for-byte; production leaves it unset. A
/// value that is present but not a `u64` is rejected as invalid input rather
/// than silently falling back to the system clock, which would make a snapshot
/// run non-deterministic for a hard-to-diagnose reason.
pub(crate) const RUN_CLOCK_EPOCH_ENV: &str = "TOVEN_CLOCK_EPOCH";

/// Resolve the wall clock the CLI mints identifiers from.
///
/// A [`FixedClock`] when [`RUN_CLOCK_EPOCH_ENV`] pins an epoch second (the
/// deterministic test/snapshot seam), else the real [`system_clock`]. Injecting
/// the clock here keeps the wall clock out of the call sites, which reach for a
/// resolved [`Clock`](rskit_util::time::Clock) rather than `SystemTime::now()`
/// directly. Fails when the env var is present but not a UTF-8 `u64`.
pub(crate) fn resolve_clock() -> AppResult<SharedClock> {
    let raw = match std::env::var(RUN_CLOCK_EPOCH_ENV) {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(AppError::invalid_input(
                RUN_CLOCK_EPOCH_ENV,
                "expected a u64 epoch second, got a non-UTF-8 value",
            ));
        }
    };
    resolve_clock_from(raw.as_deref())
}

/// Pure core of [`resolve_clock`]: map the raw env value to a clock.
///
/// `None` (var unset) yields the real [`system_clock`]. `Some` is the
/// deterministic seam: a `u64` epoch second pins a [`FixedClock`], while any
/// other value is rejected as invalid input rather than silently falling back
/// to the system clock. Kept env-free so it is unit-testable without touching
/// the process environment.
fn resolve_clock_from(raw: Option<&str>) -> AppResult<SharedClock> {
    let Some(raw) = raw else {
        return Ok(system_clock());
    };
    let trimmed = raw.trim();
    let epoch_seconds = trimmed.parse::<u64>().map_err(|error| {
        AppError::invalid_input(
            RUN_CLOCK_EPOCH_ENV,
            format!("expected a u64 epoch second, got {trimmed:?}: {error}"),
        )
    })?;
    Ok(Arc::new(FixedClock::new(epoch_seconds, 0)))
}

/// A run identifier echoed into the emitted event stream.
///
/// The injected [`Clock`](rskit_util::time::Clock)'s wall second, which under a
/// [`RUN_CLOCK_EPOCH_ENV`]-pinned [`FixedClock`] is fully deterministic — that
/// is what lets an e2e/snapshot harness match the `jsonl` Event stream exactly.
/// `run_id` is observability-only (it is never a cache key or path, and watch
/// mode further suffixes it per iteration), so second-resolution granularity
/// under the system clock is sufficient.
pub(crate) fn new_run_id() -> AppResult<String> {
    Ok(run_id_from(resolve_clock()?.as_ref()))
}

/// Format a `run_id` from an explicit clock reading (the pure, testable core of
/// [`new_run_id`], independent of how the clock was resolved).
fn run_id_from(clock: &dyn Clock) -> String {
    format!("run-{}", clock.epoch_seconds())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{discover_config, load_project};

    #[test]
    fn explicit_config_is_returned_verbatim() {
        let path = PathBuf::from("/tmp/custom/toven.toml");
        assert_eq!(discover_config(Some(&path)).unwrap(), path);
    }

    #[test]
    fn load_project_on_a_missing_config_fails() {
        let missing = Path::new("/tmp/toven-host-missing-config/toven.toml");
        assert!(load_project(missing, &[]).is_err());
    }

    #[test]
    fn run_id_is_minted_and_prefixed() {
        // Hermetic: compose the env-free core exactly as `new_run_id` does, so the
        // assertion never depends on an ambient `TOVEN_CLOCK_EPOCH` (which could
        // otherwise fail this test if a developer/CI has it set).
        let clock = super::resolve_clock_from(None).unwrap();
        assert!(super::run_id_from(clock.as_ref()).starts_with("run-"));
    }

    #[test]
    fn clock_resolves_fixed_for_a_valid_epoch_and_errors_on_garbage() {
        use super::resolve_clock_from;
        // Unset → the real system clock (resolves without error).
        assert!(resolve_clock_from(None).is_ok());
        // A `u64` epoch (surrounding whitespace trimmed) pins the fixed clock.
        assert_eq!(
            resolve_clock_from(Some(" 1700000000 "))
                .unwrap()
                .epoch_seconds(),
            1_700_000_000
        );
        // Present but not a `u64` → invalid input, never a silent system-clock fallback
        // that would make a snapshot run non-deterministic.
        assert!(resolve_clock_from(Some("not-a-number")).is_err());
    }

    #[test]
    fn run_id_from_a_fixed_clock_is_deterministic() {
        use rskit_util::time::FixedClock;
        // A pinned clock (the `RUN_CLOCK_EPOCH_ENV` path resolves to this) yields a
        // fully deterministic id, which is what makes the jsonl Event stream
        // snapshot-stable. The monotonic reading is not part of the id.
        assert_eq!(
            super::run_id_from(&FixedClock::new(1_700_000_000, 0)),
            "run-1700000000"
        );
        assert_eq!(
            super::run_id_from(&FixedClock::new(1_700_000_042, 99)),
            "run-1700000042"
        );
    }
}
