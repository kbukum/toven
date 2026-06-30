//! Shared wiring for the command verbs: config discovery, project load, the
//! injected engine host, and reporter/cache-root resolution.
//!
//! Every verb needs the same preamble — locate `toven.toml`, load the strict
//! [`Document`], resolve the workspace root, and bind the rskit-backed git /
//! digest / probe / cache ports the engine injects. This module owns that
//! preamble so the verb modules stay focused on their projection or execution.
//! It is wiring only: it prints nothing (the reporter sinks do) and returns typed
//! data + typed errors.

use std::path::{Path, PathBuf};

use rskit_errors::{AppError, AppResult};
use toven_engine::cache;
use toven_engine::config::{CanonicalRegistry, Document, ReportFormat, load};
use toven_engine::federation::{OpenMemberVcsReaders, open_project_vcs};
use toven_engine::vcs::BaselineFlags;
use toven_model::AbsPath;
use toven_ports::Provider;

use crate::flags::{OutputKind, Verbosity};
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
        AppError::not_found(
            CONFIG_FILENAME,
            Some("no toven.toml found in the current directory or any parent"),
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

/// Canonicalize `path` to an absolute path, surfacing IO failures as typed errors.
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
}

impl Report {
    /// Resolve the reporter binding: the `--output` flag wins for the format,
    /// else the `[toven].report` document setting; `verbosity` is the resolved
    /// `-v`/`-q` level.
    #[must_use]
    pub(crate) const fn resolve(
        flag: Option<OutputKind>,
        verbosity: Verbosity,
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
        Self { format, verbosity }
    }

    /// Build the matching reporter sink at the resolved verbosity.
    ///
    /// The human sink lands on stderr (its progress/status/summary lines are
    /// diagnostics), while the Jsonl sink lands on stdout as the machine-readable
    /// projection. The verbosity filters the human reporter's rendering of the
    /// Event stream; the JSON-lines sink ignores it and always emits every event
    /// so a machine consumer sees the complete record.
    #[must_use]
    pub(crate) fn reporter(self) -> Box<dyn toven_ports::Reporter> {
        match self.format {
            Format::Human => Box::new(HumanReporter::stderr(self.verbosity)),
            Format::Jsonl => Box::new(JsonlReporter::stdout()),
        }
    }
}

/// A stable run identifier echoed into the emitted event stream.
///
/// # Errors
/// A pre-epoch system clock, which leaves no monotonic basis for a unique id.
pub(crate) fn new_run_id() -> AppResult<String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| {
            AppError::new(
                rskit_errors::ErrorCode::Internal,
                "system clock is before the Unix epoch; cannot mint a run id",
            )
            .with_cause(error)
        })?
        .as_nanos();
    Ok(format!("run-{nanos}"))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{discover_config, load_project, new_run_id};

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
        assert!(new_run_id().unwrap().starts_with("run-"));
    }
}
