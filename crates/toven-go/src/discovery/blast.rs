//! Blast-radius and resource-group annotations layered onto discovered units.
//!
//! These are go-specific defaults the planner reads back through the typed
//! [`Module::resource_group`] and [`Workspace::blast_radius`] fields: every
//! module grouped under one workspace shares the workspace's checksum file (a
//! blast-radius input) and contends on one build cache (a serialization
//! resource), so they are grouped by workspace root. The workspace-level checksum
//! differs by grouping: a `go.work` workspace pins resolved versions in
//! `go.work.sum` (and the `go.work` manifest itself selects its members), whereas
//! a lone module pins them in its own `go.sum`.

use toven_model::{Module, RepoPath, Workspace};

/// The checksum file a lone (non-`go.work`) module shares.
const MODULE_SUM_FILE: &str = "go.sum";

/// The `go.work` manifest selecting a multi-module workspace's members.
const WORK_FILE: &str = "go.work";

/// The workspace-level checksum file a `go.work` grouping shares.
const WORK_SUM_FILE: &str = "go.work.sum";

/// Stamp the default resource group (`go:<workspace-root>`) on a module so the
/// executor serializes `go` invocations that contend on one build cache.
pub(crate) fn annotate_module(module: &mut Module, workspace_root: &RepoPath) {
    module.resource_group = Some(resource_group(workspace_root));
}

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

/// The `go:<workspace-root>` resource-group label.
fn resource_group(workspace_root: &RepoPath) -> String {
    format!("go:{}", workspace_root.as_path().display())
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
    use toven_model::{
        EcosystemId, Module, ModuleRef, RepoPath, ToolchainTag, Workspace, WorkspaceId,
    };

    use super::{annotate_module, annotate_workspace};

    fn module() -> Module {
        let id = ModuleRef::new(EcosystemId::new("go").unwrap(), "app").unwrap();
        Module::new(id, RepoPath::new("app").unwrap())
    }

    #[test]
    fn module_gets_workspace_scoped_resource_group() {
        let mut m = module();
        annotate_module(&mut m, &RepoPath::new(".").unwrap());
        assert_eq!(m.resource_group.as_deref(), Some("go:."));
    }

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
