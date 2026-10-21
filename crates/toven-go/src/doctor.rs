//! What `toven doctor` checks for Go: that every Go root can select the
//! toolchain its `go.work` or `go.mod` requires.
//!
//! The installed `go` may be older than a root needs. With `GOTOOLCHAIN=auto`
//! Go switches to (and may download) the required release; with `local` it
//! refuses. Loading the module list inside each root asks Go itself, so the
//! check honours the user's `GOTOOLCHAIN` and fails with Go's own reason.

use std::collections::BTreeSet;
use std::path::Path;

use rskit_errors::AppResult;
use toven_model::{RepoPath, ToolchainTag};
use toven_ports::{CheckVerdict, DoctorScope, EcosystemCheck};

use crate::config::GoConfig;
use crate::discovery;
use crate::exec::{GO_TOOL, GoTool};
use crate::modules;

/// The doctor scope for the Go roots under `project_root`.
///
/// A root is the `go.work` directory for its members, or the module directory
/// for a module outside any workspace.
///
/// # Errors
/// The configured manifests or `go.work` cannot be resolved.
pub(crate) fn scope(config: &GoConfig, project_root: &Path, go: &GoTool) -> AppResult<DoctorScope> {
    let members = modules::go_work_members(project_root, go)?.unwrap_or_default();
    let mut roots = BTreeSet::new();
    for manifest in modules::resolve(config, project_root, go)? {
        let dir = discovery::manifest_parent(&RepoPath::new(manifest)?)?;
        roots.insert(if members.contains(&dir) {
            RepoPath::new(".")?
        } else {
            dir
        });
    }
    let mut scope = DoctorScope::new(vec![ToolchainTag::new(GO_TOOL)]);
    scope.checks = roots.into_iter().map(toolchain_check).collect();
    Ok(scope)
}

/// `go list -m` must succeed inside `root`: loading the root's `go.mod` or
/// `go.work` enforces its required toolchain (`go version` does not).
fn toolchain_check(root: RepoPath) -> EcosystemCheck {
    let id = if root.as_path() == Path::new(".") {
        "toolchain".to_string()
    } else {
        format!("{root}/toolchain")
    };
    EcosystemCheck::new(
        id,
        CheckVerdict::Command {
            argv: vec![GO_TOOL.to_string(), "list".to_string(), "-m".to_string()],
            dir: root,
            version: None,
        },
    )
    .with_fix(
        "install the Go release the root's go.mod/go.work requires, or set GOTOOLCHAIN=auto so Go fetches it",
    )
}
