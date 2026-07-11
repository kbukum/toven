//! Shared Cargo workspace-root resolution.
//!
//! A `manifests` entry is a *workspace root* — the target of one `cargo
//! metadata` invocation — not an arbitrary `Cargo.toml`. Onboarding
//! ([`detect`](crate::detect)) and planning ([`discovery`](crate::discovery))
//! both resolve the same set through this module so the two stages never drift.
//!
//! Resolution honours the configured [`Manifests`] selection: an explicit,
//! author-frozen list, or `auto` — re-run first-level discovery every plan so a
//! workspace added later is picked up without editing config.

use std::collections::BTreeSet;
use std::path::Path;

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::safe_join;
use rskit_fs::sync_io::dir;
use rskit_git::IgnoreReader;
use rskit_git::cli::GitCli;

use crate::config::{Manifests, RustConfig};

/// The manifest filename that marks a Cargo project or workspace root.
pub(crate) const ROOT_MANIFEST: &str = "Cargo.toml";

/// The lockfile Cargo writes beside a workspace-root manifest.
const LOCKFILE: &str = "Cargo.lock";

/// Resolve the effective workspace-root manifests for `config` under
/// `project_root`.
///
/// An explicit selection is returned verbatim; `auto` re-runs first-level
/// discovery and drops any workspace named in `config.exclude`.
///
/// # Errors
/// Propagates a directory-listing, path-resolution, or git-ignore failure from
/// [`discover_manifests`].
pub(crate) fn resolve(config: &RustConfig, project_root: &Path) -> AppResult<Vec<String>> {
    match &config.manifests {
        Manifests::Explicit(list) => Ok(list.clone()),
        Manifests::Auto => {
            let discovered = discover_manifests(project_root)?;
            Ok(retain_included(discovered, &config.exclude))
        }
    }
}

/// Drop every manifest whose repo-relative path or workspace directory is named
/// in `exclude`.
fn retain_included(manifests: Vec<String>, exclude: &[String]) -> Vec<String> {
    if exclude.is_empty() {
        return manifests;
    }
    manifests
        .into_iter()
        .filter(|manifest| {
            let dir = workspace_dir(manifest);
            !exclude
                .iter()
                .any(|excluded| excluded == manifest || excluded == dir)
        })
        .collect()
}

/// The workspace directory of a repo-relative manifest (`""` for a root
/// manifest, `"core"` for `core/Cargo.toml`).
fn workspace_dir(manifest: &str) -> &str {
    manifest.rsplit_once('/').map_or("", |(parent, _)| parent)
}

/// Discover the repo-relative Cargo manifests under `project_root`.
///
/// A root `Cargo.toml` wins outright and is returned alone. Otherwise every
/// first-level subdirectory is scanned for a `<dir>/Cargo.toml`, skipping hidden
/// directories and any path ignored by Git, so a repository that groups several
/// workspaces under subdirectories is onboarded as one Rust ecosystem.
///
/// # Errors
/// Propagates a directory-listing, path-resolution, or git-ignore failure.
pub(crate) fn discover_manifests(project_root: &Path) -> AppResult<Vec<String>> {
    if manifest_exists(project_root, ROOT_MANIFEST)? {
        return Ok(vec![ROOT_MANIFEST.to_string()]);
    }

    let ignore = ignore_checker(project_root);
    let mut entries = dir::list(project_root)?;
    entries.sort_by(|a, b| a.file_name.cmp(&b.file_name));

    let mut manifests = Vec::new();
    for entry in entries {
        if !entry.is_dir {
            continue;
        }
        let name = entry.file_name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let manifest = format!("{name}/{ROOT_MANIFEST}");
        if !manifest_exists(project_root, &manifest)? {
            continue;
        }
        if is_git_ignored(ignore.as_ref(), &manifest)? {
            continue;
        }
        manifests.push(manifest);
    }
    Ok(manifests)
}

/// The repo-relative `Cargo.lock` beside a workspace-root manifest.
pub(crate) fn sibling_lockfile(manifest: &str) -> String {
    let dir = workspace_dir(manifest);
    if dir.is_empty() {
        LOCKFILE.to_string()
    } else {
        format!("{dir}/{LOCKFILE}")
    }
}

/// The repo-relative lockfiles that should enter `shared_inputs` for
/// `manifests`, sorted and de-duplicated.
///
/// A lockfile is included only when it exists on disk and is tracked by Git.
/// An absent path would hash to an empty digest and silently drop invalidation;
/// a git-ignored lockfile (e.g. a cargo-fuzz `Cargo.lock`) is regenerable and
/// may be missing in a fresh CI checkout, so it is skipped too.
///
/// # Errors
/// Propagates a git check-ignore failure inside a work tree.
pub(crate) fn existing_lockfiles(
    project_root: &Path,
    manifests: &[String],
) -> AppResult<Vec<String>> {
    let ignore = ignore_checker(project_root);
    let mut locks = BTreeSet::new();
    for manifest in manifests {
        let lock = sibling_lockfile(manifest);
        if is_git_ignored(ignore.as_ref(), &lock)? {
            continue;
        }
        if safe_join(project_root, Path::new(&lock)).is_ok_and(|path| path.is_file()) {
            locks.insert(lock);
        }
    }
    Ok(locks.into_iter().collect())
}

/// Whether the repo-relative `manifest` resolves to a regular file under `root`.
fn manifest_exists(root: &Path, manifest: &str) -> AppResult<bool> {
    match safe_join(root, Path::new(manifest)) {
        Ok(path) => Ok(path.is_file()),
        Err(error) => Err(AppError::new(
            ErrorCode::Internal,
            format!("failed to resolve manifest path '{manifest}'"),
        )
        .with_cause(error)),
    }
}

/// A Git ignore checker rooted at `project_root`, or `None` when the root is not
/// inside a Git work tree (no ignore information is available, so nothing is
/// filtered).
fn ignore_checker(project_root: &Path) -> Option<GitCli> {
    rskit_git::discover(project_root)
        .ok()
        .map(|_| GitCli::new(project_root.to_path_buf()))
}

/// Whether `manifest` is ignored by Git. With no checker (not a Git repo) every
/// path is included; a genuine check-ignore failure inside a repo propagates.
fn is_git_ignored(checker: Option<&GitCli>, manifest: &str) -> AppResult<bool> {
    checker.map_or(Ok(false), |git| git.is_ignored(manifest))
}

#[cfg(test)]
mod tests {
    use super::{existing_lockfiles, retain_included, sibling_lockfile, workspace_dir};

    #[test]
    fn workspace_dir_of_root_manifest_is_empty() {
        assert_eq!(workspace_dir("Cargo.toml"), "");
        assert_eq!(workspace_dir("core/Cargo.toml"), "core");
    }

    #[test]
    fn sibling_lockfile_tracks_the_manifest_directory() {
        assert_eq!(sibling_lockfile("Cargo.toml"), "Cargo.lock");
        assert_eq!(sibling_lockfile("core/Cargo.toml"), "core/Cargo.lock");
    }

    #[test]
    fn exclude_drops_by_directory_or_manifest_path() {
        let discovered = vec![
            "contrib/Cargo.toml".to_string(),
            "core/Cargo.toml".to_string(),
            "fuzz/Cargo.toml".to_string(),
        ];
        let kept = retain_included(discovered, &["fuzz".to_string()]);
        assert_eq!(kept, ["contrib/Cargo.toml", "core/Cargo.toml"]);
    }

    #[test]
    fn existing_lockfiles_skips_absent_siblings() {
        let workspace = toven_testkit::TestWorkspace::new("rust-lockfiles");
        workspace
            .write_file("core/Cargo.toml", b"[workspace]\n")
            .unwrap();
        workspace
            .write_file("core/Cargo.lock", b"# lock\n")
            .unwrap();
        // A workspace with no lockfile contributes nothing.
        workspace
            .write_file("fuzz/Cargo.toml", b"[workspace]\n")
            .unwrap();

        let locks = existing_lockfiles(
            workspace.path(),
            &["core/Cargo.toml".to_string(), "fuzz/Cargo.toml".to_string()],
        )
        .expect("lockfile probe");
        assert_eq!(locks, ["core/Cargo.lock"]);
    }
}
