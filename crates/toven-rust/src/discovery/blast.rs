//! Blast-radius and resource-group annotations layered onto discovered units.
//!
//! These are cargo-specific defaults the planner reads back through opaque
//! [`Metadata`]: every member of a Cargo workspace shares one `Cargo.lock` (a
//! blast-radius input) and one `target/` directory (a serialization resource),
//! so they are grouped by workspace root.

use serde_json::Value;
use toven_model::{Module, RepoPath, Workspace};

/// Metadata key carrying a module's default serialization resource group.
pub(crate) const RESOURCE_GROUP_KEY: &str = "resource_group";

/// Metadata key carrying a workspace's blast-radius input globs.
pub(crate) const BLAST_RADIUS_KEY: &str = "blast_radius";

/// The lockfile every member of a Cargo workspace shares.
const LOCKFILE: &str = "Cargo.lock";

/// Stamp the default `resource_group` (`cargo:<workspace-root>`) on a module so
/// the executor serializes cargo invocations that contend on one `target/` dir.
pub(crate) fn annotate_module(module: &mut Module, workspace_root: &RepoPath) {
    module.metadata.insert(
        RESOURCE_GROUP_KEY.to_string(),
        Value::String(resource_group(workspace_root)),
    );
}

/// Stamp the workspace-wide blast-radius globs (the shared `Cargo.lock`) so a
/// lockfile change invalidates every member of the workspace.
pub(crate) fn annotate_workspace(workspace: &mut Workspace, workspace_root: &RepoPath) {
    let globs = vec![Value::String(lockfile_glob(workspace_root))];
    workspace
        .metadata
        .insert(BLAST_RADIUS_KEY.to_string(), Value::Array(globs));
}

/// The `cargo:<workspace-root>` resource-group label.
fn resource_group(workspace_root: &RepoPath) -> String {
    format!("cargo:{}", workspace_root.as_path().display())
}

/// The repo-relative `Cargo.lock` glob for a workspace root.
fn lockfile_glob(workspace_root: &RepoPath) -> String {
    let label = workspace_root.as_path().display().to_string();
    if label == "." {
        LOCKFILE.to_string()
    } else {
        format!("{label}/{LOCKFILE}")
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use toven_model::EcosystemId;
    use toven_model::{Module, ModuleRef, RepoPath, ToolchainTag, Workspace, WorkspaceId};

    use super::{BLAST_RADIUS_KEY, RESOURCE_GROUP_KEY, annotate_module, annotate_workspace};

    fn module() -> Module {
        let id = ModuleRef::new(EcosystemId::new("rust").unwrap(), "app").unwrap();
        Module::new(id, RepoPath::new("crates/app").unwrap())
    }

    #[test]
    fn module_gets_workspace_scoped_resource_group() {
        let mut m = module();
        annotate_module(&mut m, &RepoPath::new(".").unwrap());
        assert_eq!(
            m.metadata.get(RESOURCE_GROUP_KEY),
            Some(&Value::String("cargo:.".to_string()))
        );
    }

    #[test]
    fn workspace_gets_lockfile_blast_radius() {
        let mut ws = Workspace::new(
            WorkspaceId::new("rust").unwrap(),
            RepoPath::new(".").unwrap(),
            ToolchainTag::new("cargo"),
        );
        annotate_workspace(&mut ws, &RepoPath::new(".").unwrap());
        assert_eq!(
            ws.metadata.get(BLAST_RADIUS_KEY),
            Some(&Value::Array(vec![Value::String("Cargo.lock".to_string())]))
        );
    }

    #[test]
    fn nested_workspace_lockfile_is_prefixed() {
        let mut ws = Workspace::new(
            WorkspaceId::new("rust:contrib").unwrap(),
            RepoPath::new("contrib").unwrap(),
            ToolchainTag::new("cargo"),
        );
        annotate_workspace(&mut ws, &RepoPath::new("contrib").unwrap());
        assert_eq!(
            ws.metadata.get(BLAST_RADIUS_KEY),
            Some(&Value::Array(vec![Value::String(
                "contrib/Cargo.lock".to_string()
            )]))
        );
    }
}
