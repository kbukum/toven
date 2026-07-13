//! `go mod edit -json` discovery: read each configured `go.mod` offline, parse
//! its module path and requires, auto-detect a root `go.work` purely to group
//! members into one workspace, and fold everything into the unified
//! `{workspaces, modules, edges}` [`DiscoverResponse`].
//!
//! Every `go` invocation goes through `rskit-process` (captured, bounded,
//! timed-out — never a shell string). `go mod edit -json` / `go work edit -json`
//! only parse the manifest text into JSON: no module-graph resolution, no
//! network, fully deterministic. In-repo `require`s whose target resolves to a
//! discovered module become intra-ecosystem [`Edge`]s, so a Go project spanning
//! several modules (with or without a `go.work`) surfaces as one module + edge
//! set.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::safe_join;
use rskit_process::ProcessSpec;
use serde::Deserialize;
use toven_model::{
    DepKind, EcosystemId, Edge, Module, ModuleRef, RepoPath, ToolchainTag, Workspace, WorkspaceId,
};
use toven_ports::{DiscoverRequest, DiscoverResponse};

use crate::config::GoConfig;
use crate::discovery::blast;
use crate::exec::{GO_TOOL, run_go_json};
use crate::modules;

/// The `Module` field of `go mod edit -json` output.
#[derive(Debug, Deserialize)]
struct GoModuleField {
    #[serde(rename = "Path")]
    path: String,
}

/// A single `require` entry of `go mod edit -json` output.
#[derive(Debug, Deserialize)]
struct GoRequire {
    #[serde(rename = "Path")]
    path: String,
}

/// The subset of `go mod edit -json` output the adapter consumes.
#[derive(Debug, Deserialize)]
struct GoModEdit {
    #[serde(rename = "Module")]
    module: GoModuleField,
    #[serde(rename = "Require")]
    require: Option<Vec<GoRequire>>,
}

/// Discover all Go modules, workspaces, and edges under `request.project_root`.
pub(crate) fn discover(
    config: &GoConfig,
    request: &DiscoverRequest,
) -> AppResult<DiscoverResponse> {
    let ecosystem = go_id()?;
    let project_root = request.project_root.as_path();

    let work_members = modules::go_work_members(project_root)?;
    let manifests = modules::resolve(config, project_root)?;

    let mut workspaces: BTreeMap<WorkspaceId, Workspace> = BTreeMap::new();
    let mut modules: BTreeMap<ModuleRef, Module> = BTreeMap::new();
    let mut by_path: BTreeMap<String, ModuleRef> = BTreeMap::new();
    let mut requires: Vec<(ModuleRef, String)> = Vec::new();

    for manifest in &manifests {
        let edit = run_go_mod_edit(project_root, manifest)?;
        let module_path = module_path(&edit, manifest)?;
        let manifest_path = RepoPath::new(Path::new(manifest))?;
        let module_root = manifest_parent(&manifest_path)?;
        let id = ModuleRef::new(ecosystem.clone(), module_name(&module_root, &module_path))?;

        if let Some(existing) = modules.get(&id) {
            return Err(AppError::new(
                ErrorCode::Conflict,
                format!(
                    "duplicate module '{id}': both manifest '{}' and '{}' resolve to the same \
                     name; module names (the module's repo-relative directory) must be unique",
                    existing
                        .manifest
                        .as_ref()
                        .map_or_else(|| existing.root.to_string(), ToString::to_string),
                    manifest_path
                ),
            ));
        }

        let (workspace_id, workspace_root, is_work) = match &work_members {
            Some(members) if members.contains(&module_root) => (
                workspace_id_for(&RepoPath::new(".")?)?,
                RepoPath::new(".")?,
                true,
            ),
            _ => (workspace_id_for(&module_root)?, module_root.clone(), false),
        };

        if let std::collections::btree_map::Entry::Vacant(slot) =
            workspaces.entry(workspace_id.clone())
        {
            let mut workspace = Workspace::new(
                workspace_id.clone(),
                workspace_root.clone(),
                ToolchainTag::new(GO_TOOL),
            );
            blast::annotate_workspace(&mut workspace, &workspace_root, is_work);
            slot.insert(workspace);
        }

        let mut module = Module::new(id.clone(), module_root);
        module.package = Some(module_path.clone());
        module.manifest = Some(manifest_path);
        module.workspace = Some(workspace_id);
        modules.insert(id.clone(), module);
        by_path.insert(module_path, id.clone());

        if let Some(reqs) = edit.require {
            for req in reqs {
                requires.push((id.clone(), req.path));
            }
        }
    }

    let edges = build_edges(&modules, &by_path, &requires);

    let mut response = DiscoverResponse::new(ecosystem);
    response.schema_version = request.schema_version;
    response.workspaces = workspaces.into_values().collect();
    response.modules = modules.into_values().collect();
    response.edges = edges;
    Ok(response)
}

/// The canonical Go ecosystem id.
fn go_id() -> AppResult<EcosystemId> {
    EcosystemId::new("go")
}

/// Run `go mod edit -json` for one manifest and parse its JSON output.
fn run_go_mod_edit(project_root: &Path, manifest: &str) -> AppResult<GoModEdit> {
    let manifest_abs = safe_join(project_root, Path::new(manifest)).map_err(|error| {
        AppError::invalid_input(
            "ecosystems.go.modules",
            format!("manifest '{manifest}' escapes the project root: {error}"),
        )
    })?;

    let spec = ProcessSpec::new(GO_TOOL)
        .arg("mod")
        .arg("edit")
        .arg("-json")
        .arg(&manifest_abs)
        .dir(project_root);
    let stdout = run_go_json(&spec, &format!("go mod edit for '{manifest}'"))?;
    rskit_codec::decode::<GoModEdit>(&rskit_codec::JsonCodec::default(), &stdout).map_err(|error| {
        AppError::new(
            ErrorCode::InvalidFormat,
            format!("failed to parse `go mod edit -json` output for '{manifest}'"),
        )
        .with_cause(error)
    })
}

/// Extract the module path, rejecting an empty one.
fn module_path(edit: &GoModEdit, manifest: &str) -> AppResult<String> {
    let path = edit.module.path.trim();
    if path.is_empty() {
        return Err(AppError::new(
            ErrorCode::InvalidFormat,
            format!("`go.mod` '{manifest}' declares an empty module path"),
        ));
    }
    Ok(path.to_string())
}

/// The module identity, unique within the repo and safe as a path segment.
///
/// A module's repo-relative root directory is its natural unique key (two
/// modules cannot share a directory), so nested modules use it with the path
/// separators folded to `-` (`database/testutil` → `database-testutil`,
/// `cache/redis` → `cache-redis`). Deriving identity from the directory — not
/// the final `go.mod` path segment — is what keeps sibling leaves like
/// `connect/testutil` and `git/testutil` from collapsing onto one name.
///
/// The repository-root module (directory `.`) instead takes the final segment
/// of its module path, with a Go major-version suffix stripped, so it reads as
/// the project name (`github.com/kbukum/gokit` → `gokit`,
/// `example.com/svc/api/v2` → `api`).
fn module_name(module_root: &RepoPath, module_path: &str) -> String {
    let root = module_root.as_path();
    if root.as_os_str().is_empty() || root == Path::new(".") {
        return root_module_name(module_path);
    }
    root.to_string_lossy().replace(['/', '\\'], "-")
}

/// The repository-root module name: the final meaningful segment of its module
/// path, with a Go major-version suffix stripped (`.../api/v2` → `api`).
fn root_module_name(module_path: &str) -> String {
    let mut segments = module_path.rsplit('/');
    let last = segments.next().unwrap_or(module_path);
    if is_major_version_suffix(last)
        && let Some(parent) = segments.next()
    {
        return parent.to_string();
    }
    last.to_string()
}

/// A Go major-version path suffix is `v` followed by an unpadded integer of two
/// or more (`v2`, `v10`); `v0`/`v1` are never written as suffixes.
/// See <https://go.dev/ref/mod#major-version-suffixes>.
fn is_major_version_suffix(segment: &str) -> bool {
    let Some(digits) = segment.strip_prefix('v') else {
        return false;
    };
    if digits.is_empty() || (digits.len() > 1 && digits.starts_with('0')) {
        return false;
    }
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    digits.parse::<u64>().is_ok_and(|major| major >= 2)
}

/// Turn the collected requires into intra-ecosystem edges, keeping only those
/// whose required module path resolves to a discovered module.
fn build_edges(
    modules: &BTreeMap<ModuleRef, Module>,
    by_path: &BTreeMap<String, ModuleRef>,
    requires: &[(ModuleRef, String)],
) -> Vec<Edge> {
    let mut edges: BTreeSet<Edge> = BTreeSet::new();
    for (from, required_path) in requires {
        if let Some(to) = by_path.get(required_path)
            && from != to
            && modules.contains_key(to)
            && modules.contains_key(from)
        {
            edges.insert(Edge::new(from.clone(), to.clone(), DepKind::Normal));
        }
    }
    edges.into_iter().collect()
}

/// The repo-relative directory containing a manifest (the module root).
fn manifest_parent(manifest: &RepoPath) -> AppResult<RepoPath> {
    match manifest.as_path().parent() {
        Some(parent) if !parent.as_os_str().is_empty() => RepoPath::new(parent),
        _ => RepoPath::new("."),
    }
}

/// Derive a stable [`WorkspaceId`] from a workspace's repo-relative root.
fn workspace_id_for(root: &RepoPath) -> AppResult<WorkspaceId> {
    let label = root.as_path().to_string_lossy();
    let id = if label == "." {
        GO_TOOL.to_string()
    } else {
        format!("{GO_TOOL}:{}", PathBuf::from(label.as_ref()).display())
    };
    WorkspaceId::new(id)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use toven_model::{EcosystemId, Module, ModuleRef, RepoPath};

    use super::{build_edges, manifest_parent, module_name, workspace_id_for};

    fn id(name: &str) -> ModuleRef {
        ModuleRef::new(EcosystemId::new("go").unwrap(), name).unwrap()
    }

    #[test]
    fn root_module_uses_module_path_leaf() {
        let root = RepoPath::new(".").unwrap();
        assert_eq!(module_name(&root, "github.com/kbukum/gokit"), "gokit");
        assert_eq!(module_name(&root, "example.com/svc/api/v2"), "api");
        assert_eq!(module_name(&root, "solo"), "solo");
    }

    #[test]
    fn nested_module_uses_directory_so_leaf_collisions_stay_distinct() {
        let connect = RepoPath::new("connect/testutil").unwrap();
        let git = RepoPath::new("git/testutil").unwrap();
        assert_eq!(
            module_name(&connect, "github.com/kbukum/gokit/connect/testutil"),
            "connect-testutil"
        );
        assert_eq!(
            module_name(&git, "github.com/kbukum/gokit/git/testutil"),
            "git-testutil"
        );
        assert_ne!(
            module_name(&connect, "github.com/kbukum/gokit/connect/testutil"),
            module_name(&git, "github.com/kbukum/gokit/git/testutil")
        );
    }

    #[test]
    fn manifest_parent_is_the_module_root() {
        let manifest = RepoPath::new("app/go.mod").unwrap();
        assert_eq!(
            manifest_parent(&manifest).unwrap(),
            RepoPath::new("app").unwrap()
        );
    }

    #[test]
    fn root_manifest_parent_is_dot() {
        let manifest = RepoPath::new("go.mod").unwrap();
        assert_eq!(
            manifest_parent(&manifest).unwrap(),
            RepoPath::new(".").unwrap()
        );
    }

    #[test]
    fn workspace_id_uses_root_label() {
        assert_eq!(
            workspace_id_for(&RepoPath::new(".").unwrap())
                .unwrap()
                .as_str(),
            "go"
        );
        assert_eq!(
            workspace_id_for(&RepoPath::new("svc").unwrap())
                .unwrap()
                .as_str(),
            "go:svc"
        );
    }

    #[test]
    fn edges_keep_only_in_repo_requires() {
        let app = id("app");
        let core = id("core");
        let mut modules = BTreeMap::new();
        modules.insert(
            app.clone(),
            Module::new(app.clone(), RepoPath::new("app").unwrap()),
        );
        modules.insert(
            core.clone(),
            Module::new(core.clone(), RepoPath::new("core").unwrap()),
        );

        let mut by_path = BTreeMap::new();
        by_path.insert("example.com/app".to_string(), app.clone());
        by_path.insert("example.com/core".to_string(), core.clone());

        let requires = vec![
            (app.clone(), "example.com/core".to_string()),
            (app.clone(), "golang.org/x/text".to_string()),
        ];
        let edges = build_edges(&modules, &by_path, &requires);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].from.module, app);
        assert_eq!(edges[0].to.module, core);
    }
}
