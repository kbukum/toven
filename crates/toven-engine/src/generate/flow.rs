//! The generate flow: assemble → probe → merge → render → (optional) write.

use std::path::{Path, PathBuf};

use rskit_errors::AppResult;
use rskit_fs::sync_io::file::{read_string_bounded, write_atomic_replace};
use toven_model::EcosystemId;
use toven_ports::{DriverLocator, DriverScaffolder, Provider};

use super::merge::{self, MergeResult};
use super::probe::{self, ProcessDriverScaffolder};
use super::render;
use crate::federation::PathDriverLocator;

/// Upper bound on an existing `toven.toml` read for the additive re-run merge.
const MAX_CONFIG_BYTES: u64 = 8 * 1024 * 1024;

/// The temp-file prefix for the atomic config write.
const WRITE_PREFIX: &str = "toven-generate";

/// The result of one `toven generate` run.
///
/// The rendered document is always returned; whether it was written to disk
/// depends on the `write` flag. Diagnostics (skipped/ineffective sections) are
/// carried separately so the CLI can route them to stderr while the document
/// goes to the file or stdout.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct GeneratedDocument {
    /// The resolved `<root>/toven.toml` path.
    pub path: PathBuf,
    /// The rendered, format-preserving document text.
    pub rendered: String,
    /// Whether the document was written to [`path`](Self::path).
    pub written: bool,
    /// Whether [`path`](Self::path) did not exist before this run created it.
    ///
    /// Distinguishes a brand-new file (even a `[project]`-only one with no
    /// detected ecosystems) from an additive re-run that added no sections, so
    /// the CLI does not report a fresh write as "up to date".
    pub created: bool,
    /// Ecosystem sections added by this run.
    pub added: Vec<EcosystemId>,
    /// Ecosystem sections regenerated because `--force <id>` named them.
    pub regenerated: Vec<EcosystemId>,
    /// Human-facing diagnostics (existing sections skipped on a plain re-run).
    pub warnings: Vec<String>,
}

/// Detect ecosystems under `root` and produce (and optionally write) a
/// `toven.toml`, using the production driver-scaffolder and `PATH` locator.
///
/// `providers` is the in-proc bootstrap set; `force` regenerates one section;
/// `write` persists the document atomically (otherwise the caller prints it).
///
/// # Errors
/// Propagates a provider/driver scaffold failure, an unreadable/invalid existing
/// config, or a failed atomic write.
pub fn generate(
    root: &Path,
    providers: &[&dyn Provider],
    force: Option<&str>,
    write: bool,
) -> AppResult<GeneratedDocument> {
    generate_with(
        root,
        providers,
        &ProcessDriverScaffolder::new(),
        &PathDriverLocator::new(),
        force,
        write,
    )
}

/// The injectable core of [`generate`]: the driver-scaffolder and locator are
/// parameters so tests drive the bootstrap probe without spawning subprocesses.
///
/// # Errors
/// See [`generate`].
pub fn generate_with(
    root: &Path,
    providers: &[&dyn Provider],
    scaffolder: &dyn DriverScaffolder,
    locator: &dyn DriverLocator,
    force: Option<&str>,
    write: bool,
) -> AppResult<GeneratedDocument> {
    let fragments = probe::probe(providers, scaffolder, locator, root)?;
    let config_path = root.join("toven.toml");
    let pre_existed = config_path.is_file();

    let result = if pre_existed {
        let existing = read_string_bounded(&config_path, MAX_CONFIG_BYTES)?;
        merge::merge(&existing, &fragments, force)?
    } else {
        let (text, added) = render::first_run(&project_name(root), &fragments)?;
        let warnings = force_without_target(force, &added);
        MergeResult {
            text,
            added,
            regenerated: Vec::new(),
            warnings,
        }
    };

    if write {
        write_atomic_replace(&config_path, result.text.as_bytes(), WRITE_PREFIX)?;
    }

    Ok(GeneratedDocument {
        path: config_path,
        rendered: result.text,
        written: write,
        created: write && !pre_existed,
        added: result.added,
        regenerated: result.regenerated,
        warnings: result.warnings,
    })
}

/// Derive the `[project]` name from the root directory's file name, falling back
/// to a stable placeholder when the root has no nameable component (e.g. `/`).
fn project_name(root: &Path) -> String {
    root.canonicalize()
        .ok()
        .as_deref()
        .and_then(Path::file_name)
        .or_else(|| root.file_name())
        .and_then(|name| name.to_str())
        .map_or_else(|| "workspace".to_string(), ToString::to_string)
}

/// Warn when `--force <id>` was given on a first run but nothing detected that id.
fn force_without_target(force: Option<&str>, added: &[EcosystemId]) -> Vec<String> {
    match force {
        Some(id) if !added.iter().any(|eco| eco.as_str() == id) => {
            vec![merge::force_no_effect_hint(id)]
        }
        _ => Vec::new(),
    }
}
