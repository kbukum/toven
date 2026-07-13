//! Go module-set resolution.
//!
//! A `modules` entry is a repo-relative `go.mod` — one module root. Onboarding
//! ([`render`](crate::render) writes `"auto"`) and planning
//! ([`discovery`](crate::discovery)) both resolve the effective set through this
//! module so the two stages never drift.
//!
//! Resolution honours the configured [`Modules`](crate::config::Modules)
//! selection: an explicit, author-frozen list, or `auto` — re-derive the set on
//! every plan so a module added to `go.work` (or a new nested `go.mod`) is
//! picked up without editing config. `auto` prefers a root `go.work`'s member
//! list (the workspace's own source of truth, at any depth); with no `go.work`
//! it falls back to the root `go.mod` plus every first-level nested `go.mod`.

use std::collections::BTreeSet;
use std::path::Path;

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::safe_join;
use rskit_fs::sync_io::dir;
use rskit_git::IgnoreReader;
use rskit_git::cli::GitCli;
use rskit_process::ProcessSpec;
use serde::Deserialize;
use toven_model::RepoPath;

use crate::config::{GoConfig, Modules};
use crate::detect::ROOT_MANIFEST;
use crate::exec::{GO_TOOL, run_go_json};

/// The workspace manifest that groups several modules into one build unit.
pub(crate) const WORK_MANIFEST: &str = "go.work";

/// A single `use` entry of `go work edit -json` output.
#[derive(Debug, Deserialize)]
struct GoWorkUse {
    #[serde(rename = "DiskPath")]
    disk_path: String,
}

/// The subset of `go work edit -json` output the adapter consumes.
#[derive(Debug, Deserialize)]
struct GoWorkEdit {
    #[serde(rename = "Use")]
    use_dirs: Option<Vec<GoWorkUse>>,
}

/// Resolve the effective repo-relative `go.mod` manifests for `config` under
/// `project_root`.
///
/// An explicit selection is returned verbatim; `auto` re-derives the set from
/// `go.work` (or the on-disk `go.mod` layout).
///
/// # Errors
/// Propagates a `go.work` read/parse failure or a directory-listing,
/// path-resolution, or git-ignore failure from [`discover_modules`].
pub(crate) fn resolve(config: &GoConfig, project_root: &Path) -> AppResult<Vec<String>> {
    match &config.modules {
        Modules::Explicit(list) => Ok(list.clone()),
        Modules::Auto => discover_modules(project_root),
    }
}

/// Discover the repo-relative `go.mod` manifests under `project_root`.
///
/// A root `go.work` wins: its member directories (at any depth) map to their
/// `go.mod`. With no `go.work`, the root `go.mod` (when present) plus every
/// non-hidden, non-git-ignored first-level `<dir>/go.mod` are returned, so a
/// multi-module repo without a workspace file still onboards as one ecosystem.
///
/// # Errors
/// Propagates a `go.work` read/parse, directory-listing, path-resolution, or
/// git-ignore failure.
pub(crate) fn discover_modules(project_root: &Path) -> AppResult<Vec<String>> {
    if let Some(members) = go_work_members(project_root)? {
        return Ok(members
            .iter()
            .map(manifest_in)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect());
    }
    nested_modules(project_root)
}

/// The root `go.mod` plus every first-level nested `<dir>/go.mod`, skipping
/// hidden and git-ignored directories.
fn nested_modules(project_root: &Path) -> AppResult<Vec<String>> {
    let ignore = ignore_checker(project_root);
    let mut manifests = BTreeSet::new();

    if manifest_exists(project_root, ROOT_MANIFEST)? {
        manifests.insert(ROOT_MANIFEST.to_string());
    }

    for entry in dir::list(project_root)? {
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
        manifests.insert(manifest);
    }
    Ok(manifests.into_iter().collect())
}

/// Detect a root `go.work` and, if present, return the repo-relative roots of
/// its member modules; `None` when there is no workspace file.
///
/// Shared by resolution (which modules exist) and discovery grouping (which
/// modules share one workspace), so the two never disagree.
///
/// # Errors
/// Propagates a path-resolution failure or a `go work edit -json`
/// invocation/parse failure, and fails when the workspace file declares no
/// `use` modules (an empty set would silently discover zero modules).
pub(crate) fn go_work_members(project_root: &Path) -> AppResult<Option<BTreeSet<RepoPath>>> {
    let work_abs = safe_join(project_root, Path::new(WORK_MANIFEST)).map_err(|error| {
        AppError::new(ErrorCode::Internal, "failed to resolve go.work path").with_cause(error)
    })?;
    if !work_abs.is_file() {
        return Ok(None);
    }

    let spec = ProcessSpec::new(GO_TOOL)
        .arg("work")
        .arg("edit")
        .arg("-json")
        .arg(&work_abs)
        .dir(project_root);
    let stdout = run_go_json(&spec, "go work edit")?;
    let edit: GoWorkEdit = rskit_codec::decode(&rskit_codec::JsonCodec::default(), &stdout)
        .map_err(|error| {
            AppError::new(
                ErrorCode::InvalidFormat,
                "failed to parse `go work edit -json` output",
            )
            .with_cause(error)
        })?;

    let mut members = BTreeSet::new();
    if let Some(uses) = edit.use_dirs {
        for entry in uses {
            members.insert(RepoPath::new(Path::new(&entry.disk_path))?);
        }
    }
    if members.is_empty() {
        return Err(AppError::new(
            ErrorCode::InvalidInput,
            format!(
                "`{WORK_MANIFEST}` declares no `use` modules; add a `use` directive or remove the workspace file"
            ),
        ));
    }
    Ok(Some(members))
}

/// The repo-relative `go.mod` inside a member root (`.` → `go.mod`,
/// `agent` → `agent/go.mod`).
fn manifest_in(member: &RepoPath) -> String {
    let root = member.as_path();
    if root.as_os_str().is_empty() || root == Path::new(".") {
        ROOT_MANIFEST.to_string()
    } else {
        format!("{}/{ROOT_MANIFEST}", root.display())
    }
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
/// inside a Git work tree (no ignore information is available).
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
    use rskit_errors::ErrorCode;
    use toven_model::RepoPath;

    use super::{discover_modules, manifest_in};

    #[test]
    fn manifest_in_maps_root_and_nested_members() {
        assert_eq!(manifest_in(&RepoPath::new(".").unwrap()), "go.mod");
        assert_eq!(
            manifest_in(&RepoPath::new("agent").unwrap()),
            "agent/go.mod"
        );
        assert_eq!(
            manifest_in(&RepoPath::new("cache/redis").unwrap()),
            "cache/redis/go.mod"
        );
    }

    #[test]
    fn go_work_members_map_to_manifests_at_any_depth() {
        let workspace = toven_testkit::TestWorkspace::new("go-work-members");
        workspace
            .write_file(
                "go.work",
                b"go 1.26\n\nuse (\n\t.\n\t./cache\n\t./cache/redis\n)\n",
            )
            .unwrap();
        workspace.write_file("go.mod", b"module ex\n").unwrap();
        workspace
            .write_file("cache/go.mod", b"module ex/cache\n")
            .unwrap();
        workspace
            .write_file("cache/redis/go.mod", b"module ex/cache/redis\n")
            .unwrap();

        let manifests = discover_modules(workspace.path()).expect("discover");
        assert_eq!(manifests, ["cache/go.mod", "cache/redis/go.mod", "go.mod"]);
    }

    #[test]
    fn nested_fallback_finds_first_level_modules_without_go_work() {
        let workspace = toven_testkit::TestWorkspace::new("go-nested");
        workspace.write_file("go.mod", b"module ex\n").unwrap();
        workspace
            .write_file("auth/go.mod", b"module ex/auth\n")
            .unwrap();
        workspace
            .write_file("authz/go.mod", b"module ex/authz\n")
            .unwrap();

        let manifests = discover_modules(workspace.path()).expect("discover");
        assert_eq!(manifests, ["auth/go.mod", "authz/go.mod", "go.mod"]);
    }

    #[test]
    fn go_work_without_use_entries_fails_fast() {
        let workspace = toven_testkit::TestWorkspace::new("go-work-empty");
        workspace.write_file("go.work", b"go 1.26\n").unwrap();
        workspace.write_file("go.mod", b"module ex\n").unwrap();

        let error = discover_modules(workspace.path()).expect_err("empty go.work is rejected");
        assert_eq!(error.code(), ErrorCode::InvalidInput);
    }
}
