//! Git changed-path discovery for affected detection.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use rskit_git::{Differ, FileStatus};

use crate::{
    core::{AppError, AppResult, Workspace},
    engine::affected::ChangedPath,
    git::baseline::Baseline,
};

/// Compute changed workspace-relative paths from committed diff plus working-tree status.
pub(crate) fn changed_paths(
    workspace: &Workspace,
    baseline: &Baseline,
) -> AppResult<Vec<ChangedPath>> {
    let repo = rskit_git::discover(&workspace.root).map_err(|error| {
        AppError::invalid_input(
            "workspace.root",
            format!(
                "failed to discover git repository from '{}'",
                workspace.root.display()
            ),
        )
        .with_cause(error)
    })?;
    let workspace_prefix =
        rskit_git::repo_relative_path(rskit_git::Repository::root(&repo), &workspace.root)?;
    let mut paths = BTreeMap::new();
    let diff_base = if baseline.oid.is_empty() {
        baseline.revision.as_str()
    } else {
        baseline.oid.as_str()
    };

    for entry in repo.diff(diff_base, "HEAD").map_err(|error| {
        AppError::invalid_input("base", format!("failed to diff '{diff_base}' against HEAD"))
            .with_cause(error)
    })? {
        insert_repo_path(&workspace_prefix, &mut paths, entry.path);
        if matches!(entry.status, FileStatus::Deleted | FileStatus::Renamed)
            && let Some(old_path) = entry.old_path
        {
            insert_repo_path(&workspace_prefix, &mut paths, old_path);
        }
    }

    for entry in repo.status().map_err(|error| {
        AppError::invalid_input(
            "workspace.root",
            format!(
                "failed to read git status from '{}'",
                workspace.root.display()
            ),
        )
        .with_cause(error)
    })? {
        insert_repo_path(&workspace_prefix, &mut paths, entry.path);
    }

    Ok(paths.into_values().collect())
}

fn insert_repo_path(
    workspace_prefix: &Path,
    paths: &mut BTreeMap<PathBuf, ChangedPath>,
    repo_path: String,
) {
    let repo_path = PathBuf::from(repo_path);
    let workspace_path = if workspace_prefix.as_os_str().is_empty() {
        Some(repo_path)
    } else {
        repo_path
            .strip_prefix(workspace_prefix)
            .map(Path::to_path_buf)
            .ok()
    };
    if let Some(workspace_path) = workspace_path {
        paths.insert(workspace_path.clone(), ChangedPath::new(workspace_path));
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path, process::Command};

    use crate::{
        core::Workspace,
        git::{affected::changed_paths, baseline::Baseline},
    };

    #[test]
    fn includes_baseline_diff_and_worktree_status_paths() {
        let root = unique_temp_dir("git-affected");
        fs::create_dir_all(root.join("module-a/src")).unwrap();
        fs::create_dir_all(root.join("module-b/src")).unwrap();

        git(&root, ["init"]);
        git(&root, ["config", "user.email", "toven@example.invalid"]);
        git(&root, ["config", "user.name", "Toven Test"]);
        fs::write(root.join("module-a/src/lib.rs"), "pub fn base() {}\n").unwrap();
        git(&root, ["add", "."]);
        git(&root, ["commit", "-m", "base"]);
        let base = git_stdout(&root, ["rev-parse", "HEAD"]);

        fs::write(root.join("module-a/src/lib.rs"), "pub fn committed() {}\n").unwrap();
        git(&root, ["add", "."]);
        git(&root, ["commit", "-m", "committed"]);
        fs::write(root.join("module-a/src/lib.rs"), "pub fn unstaged() {}\n").unwrap();
        fs::write(root.join("module-b/src/new.rs"), "pub fn staged() {}\n").unwrap();
        git(&root, ["add", "module-b/src/new.rs"]);
        fs::write(root.join("README.md"), "# untracked\n").unwrap();

        let workspace = Workspace {
            schema: 1,
            name: "fixture".to_string(),
            root: root.clone(),
            base_ref: None,
            profiles: Vec::new(),
            dependency_overlays: Vec::new(),
        };
        let baseline = Baseline {
            provider: "explicit".to_string(),
            revision: base,
            oid: String::new(),
        };

        let paths = changed_paths(&workspace, &baseline)
            .unwrap()
            .into_iter()
            .map(|path| path.path)
            .collect::<Vec<_>>();

        assert!(paths.contains(&Path::new("module-a/src/lib.rs").to_path_buf()));
        assert!(paths.contains(&Path::new("module-b/src/new.rs").to_path_buf()));
        assert!(paths.contains(&Path::new("README.md").to_path_buf()));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn diffs_against_resolved_oid_when_revision_ref_moves() {
        let root = unique_temp_dir("git-affected-moving-ref");
        fs::create_dir_all(root.join("module-a/src")).unwrap();

        git(&root, ["init"]);
        git(&root, ["config", "user.email", "toven@example.invalid"]);
        git(&root, ["config", "user.name", "Toven Test"]);
        fs::write(root.join("module-a/src/lib.rs"), "pub fn base() {}\n").unwrap();
        git(&root, ["add", "."]);
        git(&root, ["commit", "-m", "base"]);
        let base = git_stdout(&root, ["rev-parse", "HEAD"]);
        git(&root, ["branch", "moving"]);

        fs::write(
            root.join("module-a/src/lib.rs"),
            "pub fn changed_after_base() {}\n",
        )
        .unwrap();
        git(&root, ["add", "."]);
        git(&root, ["commit", "-m", "changed"]);
        git(&root, ["branch", "-f", "moving", "HEAD"]);

        let workspace = Workspace {
            schema: 1,
            name: "fixture".to_string(),
            root: root.clone(),
            base_ref: None,
            profiles: Vec::new(),
            dependency_overlays: Vec::new(),
        };
        let baseline = Baseline {
            provider: "git-ref".to_string(),
            revision: "moving".to_string(),
            oid: base,
        };

        let changed_source = Path::new("module-a/src/lib.rs").to_path_buf();
        let contains_changed_source = changed_paths(&workspace, &baseline)
            .unwrap()
            .into_iter()
            .any(|path| path.path == changed_source);

        assert!(contains_changed_source);

        fs::remove_dir_all(root).unwrap();
    }

    fn unique_temp_dir(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "toven-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn git<const N: usize>(cwd: &Path, args: [&str; N]) {
        let output = Command::new("git")
            .current_dir(cwd)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "gc.auto=0",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_stdout<const N: usize>(cwd: &Path, args: [&str; N]) -> String {
        let output = Command::new("git")
            .current_dir(cwd)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "gc.auto=0",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }
}
