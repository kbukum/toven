//! Blast-radius annotations layered onto discovered workspaces.
//!
//! Go modules carry no serialization `resource_group`: `go` reads and writes
//! its build cache (`GOCACHE`) and module cache (`GOMODCACHE`) under file
//! locks, so `go build`/`vet`/`test` run safely in parallel across modules.
//! Leaving the group unset lets the executor give each module its own lane and
//! run the workspace's modules concurrently within their dependency waves.
//!
//! Only the workspace-level blast radius is go-specific: the planner reads it
//! back through [`Workspace::blast_radius`]. The checksum differs by grouping:
//! a `go.work` workspace pins resolved versions in `go.work.sum` (and the
//! `go.work` manifest itself selects its members), whereas a lone module pins
//! them in its own `go.sum`.

use toven_model::{RepoPath, Workspace};

/// The checksum file a lone (non-`go.work`) module shares.
const MODULE_SUM_FILE: &str = "go.sum";

/// The `go.work` manifest selecting a multi-module workspace's members.
const WORK_FILE: &str = "go.work";

/// The workspace-level checksum file a `go.work` grouping shares.
const WORK_SUM_FILE: &str = "go.work.sum";

/// Stamp the workspace-wide blast-radius globs so a checksum change invalidates
/// every member of the workspace. A `go.work` grouping (`is_work`) keys off the
/// workspace-root `go.work` manifest and its `go.work.sum`; a lone module keys
/// off its own `go.sum`.
pub(crate) fn annotate_workspace(
    workspace: &mut Workspace,
    workspace_root: &RepoPath,
    is_work: bool,
) {
    let files: &[&str] = if is_work {
        &[WORK_FILE, WORK_SUM_FILE]
    } else {
        &[MODULE_SUM_FILE]
    };
    workspace.blast_radius = files
        .iter()
        .map(|file| root_relative(workspace_root, file))
        .collect();
}

/// A workspace-root-relative glob for `file` (bare at the repo root).
fn root_relative(workspace_root: &RepoPath, file: &str) -> String {
    let label = workspace_root.as_path().display().to_string();
    if label == "." {
        file.to_string()
    } else {
        format!("{label}/{file}")
    }
}

#[cfg(test)]
mod tests {
    use toven_model::{RepoPath, ToolchainTag, Workspace, WorkspaceId};

    use super::annotate_workspace;

    #[test]
    fn workspace_gets_sum_blast_radius() {
        let mut ws = Workspace::new(
            WorkspaceId::new("go").unwrap(),
            RepoPath::new(".").unwrap(),
            ToolchainTag::new("go"),
        );
        annotate_workspace(&mut ws, &RepoPath::new(".").unwrap(), false);
        assert_eq!(ws.blast_radius, vec!["go.sum".to_string()]);
    }

    #[test]
    fn go_work_workspace_keys_off_work_sum() {
        let mut ws = Workspace::new(
            WorkspaceId::new("go").unwrap(),
            RepoPath::new(".").unwrap(),
            ToolchainTag::new("go"),
        );
        annotate_workspace(&mut ws, &RepoPath::new(".").unwrap(), true);
        assert_eq!(
            ws.blast_radius,
            vec!["go.work".to_string(), "go.work.sum".to_string()]
        );
    }

    #[test]
    fn nested_workspace_sum_is_prefixed() {
        let mut ws = Workspace::new(
            WorkspaceId::new("go:svc").unwrap(),
            RepoPath::new("svc").unwrap(),
            ToolchainTag::new("go"),
        );
        annotate_workspace(&mut ws, &RepoPath::new("svc").unwrap(), false);
        assert_eq!(ws.blast_radius, vec!["svc/go.sum".to_string()]);
    }
}
