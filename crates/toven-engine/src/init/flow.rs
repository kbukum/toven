//! The init flow: assemble → probe → merge → render → (optional) write.

use std::path::{Path, PathBuf};

use rskit_errors::AppResult;
use rskit_fs::sync_io::file::{read_string_bounded, write_atomic_replace};
use rskit_git::{BranchFilter, RefManager, RemoteManager, Repository};
use toven_model::EcosystemId;
use toven_ports::{AnswerProvider, DriverLocator, DriverWizard, Provider};

use super::merge::{self, MergeResult};
use super::probe::{self, ProcessDriverWizard};
use super::render;
use toven_engine_core::federation::PathDriverLocator;

/// Upper bound on an existing `toven.toml` read for the additive re-run merge.
const MAX_CONFIG_BYTES: u64 = 8 * 1024 * 1024;

/// The temp-file prefix for the atomic config write.
const WRITE_PREFIX: &str = "toven-init";

/// The result of one `toven init` run.
///
/// The rendered document is always returned; whether it was written to disk
/// depends on the `write` flag. Diagnostics (skipped/ineffective sections) are
/// carried separately so the CLI can route them to stderr while the document
/// goes to the file or stdout.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct InitOutcome {
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
    /// Every ecosystem detected under the root, whether or not this run added a
    /// section for it (an additive re-run detects but adds nothing). Lets the CLI
    /// report `detected: …` in both write and `--print` modes.
    pub detected: Vec<EcosystemId>,
    /// Human-facing diagnostics (existing sections skipped on a plain re-run).
    pub warnings: Vec<String>,
}

/// Detect ecosystems under `root`, run each provider's wizard (answered through
/// `answers`), and produce (and optionally write) a `toven.toml`, using the
/// production driver-wizard and `PATH` locator.
///
/// `providers` is the in-proc bootstrap set; `force` regenerates one section;
/// `write` persists the document atomically (otherwise the caller prints it).
///
/// # Errors
/// Propagates a provider/driver wizard failure, an answering failure, an
/// unreadable/invalid existing config, or a failed atomic write.
pub fn init(
    root: &Path,
    providers: &[&dyn Provider],
    answers: &dyn AnswerProvider,
    force: Option<&str>,
    write: bool,
) -> AppResult<InitOutcome> {
    init_with(
        root,
        providers,
        &ProcessDriverWizard::new(),
        &PathDriverLocator::new(),
        answers,
        force,
        write,
    )
}

/// The injectable core of [`init`]: the driver-wizard and locator are
/// parameters so tests drive the bootstrap probe without spawning subprocesses.
///
/// # Errors
/// See [`init`].
#[allow(clippy::too_many_arguments)]
pub fn init_with(
    root: &Path,
    providers: &[&dyn Provider],
    wizard: &dyn DriverWizard,
    locator: &dyn DriverLocator,
    answers: &dyn AnswerProvider,
    force: Option<&str>,
    write: bool,
) -> AppResult<InitOutcome> {
    let fragments = probe::probe(providers, wizard, locator, answers, root)?;
    let config_path = root.join("toven.toml");
    let pre_existed = config_path.is_file();

    let mut result = if pre_existed {
        let existing = read_string_bounded(&config_path, MAX_CONFIG_BYTES)?;
        merge::merge(&existing, &fragments, force)?
    } else {
        let (base_ref, base_ref_warning) = resolve_base_ref(root);
        let (text, added) = render::first_run(&project_name(root), &base_ref, &fragments)?;
        let mut warnings = force_without_target(force, &added);
        warnings.extend(base_ref_warning);
        MergeResult {
            text,
            added,
            regenerated: Vec::new(),
            warnings,
        }
    };

    if fragments.is_empty() {
        result.warnings.insert(0, no_ecosystem_hint(root));
    }

    // The full detected set (sorted, de-duplicated) — reported by the CLI in both
    // write and `--print` modes, independent of which sections this run added.
    let mut detected: Vec<EcosystemId> = fragments
        .iter()
        .map(|fragment| fragment.ecosystem.clone())
        .collect();
    detected.sort();
    detected.dedup();

    if write {
        write_atomic_replace(&config_path, result.text.as_bytes(), WRITE_PREFIX)?;
    }

    Ok(InitOutcome {
        path: config_path,
        rendered: result.text,
        written: write,
        created: write && !pre_existed,
        added: result.added,
        regenerated: result.regenerated,
        detected,
        warnings: result.warnings,
    })
}

/// The conventional change baseline used when no repository facts are available
/// (init run outside a git work tree).
const FALLBACK_BASE_REF: &str = "origin/main";

/// Resolve the `[project].base_ref` to write into a fresh config, plus an
/// optional warning to surface.
///
/// Gathers the repository's remotes and local branches and folds them into a
/// baseline via [`base_ref_from`]. Any git failure (or init outside a work tree)
/// falls back to the conventional `origin/main` with no warning — there is
/// nothing to detect there, and the value is a documented default the user can
/// edit. When the repo has no remote, a local trunk branch is written instead of
/// a dangling `origin/*` ref (which every later `affected` run would fail to
/// resolve), and the returned warning explains the substitution.
fn resolve_base_ref(root: &Path) -> (String, Option<String>) {
    let Ok(repo) = rskit_git::discover(root) else {
        return (FALLBACK_BASE_REF.to_string(), None);
    };
    let remotes: Vec<String> = repo
        .list_remotes()
        .map(|remotes| remotes.into_iter().map(|remote| remote.name).collect())
        .unwrap_or_default();
    let branches: Vec<String> = repo
        .list_branches(BranchFilter::Local)
        .map(|branches| branches.into_iter().map(|branch| branch.name).collect())
        .unwrap_or_default();
    base_ref_from(&remotes, &branches)
}

/// Pure core of [`resolve_base_ref`]: fold remote names and local branch names
/// into a `(base_ref, warning)` pair, with no git or IO.
///
/// With a remote present (`origin` preferred, else the first), the baseline is
/// `<remote>/<trunk>`. With no remote, the baseline is the bare local `<trunk>`
/// and a warning is returned so a solo repo does not silently write an
/// unresolvable `origin/*` ref. `<trunk>` prefers `main`, then `master`, then the
/// first local branch, then `main`.
fn base_ref_from(remotes: &[String], local_branches: &[String]) -> (String, Option<String>) {
    let trunk = pick_trunk(local_branches);
    pick_remote(remotes).map_or_else(
        || {
            (
                trunk.clone(),
                Some(format!(
                    "no git remote detected; wrote `base_ref = \"{trunk}\"` (a local branch). \
                     Changed-selection commands (`affected`, `--base`) compare against it — set \
                     `[project].base_ref` to your upstream branch once a remote is configured."
                )),
            )
        },
        |remote| (format!("{remote}/{trunk}"), None),
    )
}

/// Pick the remote a baseline should track: `origin` when present, else the
/// first configured remote, else `None`.
fn pick_remote(remotes: &[String]) -> Option<&str> {
    remotes
        .iter()
        .map(String::as_str)
        .find(|name| *name == "origin")
        .or_else(|| remotes.first().map(String::as_str))
}

/// Pick the trunk branch name: `main`, then `master`, then the first local
/// branch, then `main` as a last resort.
fn pick_trunk(local_branches: &[String]) -> String {
    for candidate in ["main", "master"] {
        if local_branches.iter().any(|branch| branch == candidate) {
            return candidate.to_string();
        }
    }
    local_branches
        .first()
        .cloned()
        .unwrap_or_else(|| "main".to_string())
}

/// Derive the `[project]` name, preferring the enclosing git repository's
/// top-level directory name so a nested workspace keeps the meaningful repo
/// identity, and falling back to the root directory's own name (then a stable
/// placeholder when neither yields a nameable component, e.g. `/`).
fn project_name(root: &Path) -> String {
    git_top_level_name(root)
        .or_else(|| dir_name(root))
        .unwrap_or_else(|| "workspace".to_string())
}

/// The enclosing git work-tree's top-level directory name, or `None` when
/// `root` is not inside a git repository (or the top level has no nameable
/// component).
fn git_top_level_name(root: &Path) -> Option<String> {
    let repo = rskit_git::discover(root).ok()?;
    repo.root()
        .file_name()
        .and_then(|name| name.to_str())
        .map(ToString::to_string)
}

/// The `root` directory's own name, canonicalizing first so `.` resolves to the
/// real directory rather than the literal `.`.
fn dir_name(root: &Path) -> Option<String> {
    rskit_fs::canonicalize(root)
        .ok()
        .as_deref()
        .and_then(Path::file_name)
        .or_else(|| root.file_name())
        .and_then(|name| name.to_str())
        .map(ToString::to_string)
}

/// Guidance shown when detection found no ecosystem: name the scanned root and
/// point at the two ways forward (a nested `--root`, or a manual section).
fn no_ecosystem_hint(root: &Path) -> String {
    format!(
        "no ecosystem detected under {}; the generated config has only a \
         `[project]` section. If your workspace is nested, re-run with `--root \
         <dir>`; otherwise add an `[ecosystems.<id>]` section for the toolchain \
         you use.",
        root.display()
    )
}

/// Warn when `--force <id>` was given on a first run but nothing detected that
/// id.
fn force_without_target(force: Option<&str>, added: &[EcosystemId]) -> Vec<String> {
    match force {
        Some(id) if !added.iter().any(|eco| eco.as_str() == id) => {
            vec![merge::force_no_effect_hint(id)]
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::base_ref_from;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn base_ref_prefers_origin_and_main() {
        let (base, warning) = base_ref_from(&strings(&["upstream", "origin"]), &strings(&["main"]));
        assert_eq!(base, "origin/main");
        assert!(warning.is_none());
    }

    #[test]
    fn base_ref_uses_first_remote_and_master_when_no_main() {
        let (base, warning) = base_ref_from(&strings(&["fork"]), &strings(&["master", "dev"]));
        assert_eq!(base, "fork/master");
        assert!(warning.is_none());
    }

    #[test]
    fn base_ref_without_a_remote_writes_a_local_trunk_and_warns() {
        // The cited failure: a remote-less repo must not get a dangling `origin/*`
        // ref that every later `affected` run would fail to resolve.
        let (base, warning) = base_ref_from(&[], &strings(&["main", "feature"]));
        assert_eq!(base, "main");
        let warning = warning.expect("a remote-less repo warns about the local baseline");
        assert!(warning.contains("no git remote"), "{warning}");
        assert!(warning.contains("base_ref = \"main\""), "{warning}");
    }

    #[test]
    fn base_ref_falls_back_to_main_when_nothing_is_known() {
        let (base, warning) = base_ref_from(&[], &[]);
        assert_eq!(base, "main");
        assert!(warning.is_some());
    }
}
