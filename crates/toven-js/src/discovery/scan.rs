//! Offline JavaScript discovery: fold every root and member `package.json`
//! into the unified `{workspaces, modules, edges}` [`DiscoverResponse`].
//!
//! Discovery reads manifests and checks lockfile presence directly with
//! bounded reads. It needs no `node_modules`, no network, and no package
//! manager binary, and it never spawns a process.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_model::{
    Capability, EcosystemId, Module, ModuleRef, RepoPath, ToolchainTag, Workspace, WorkspaceId,
};
use toven_ports::{DiscoverRequest, DiscoverResponse};

use crate::config::JsConfig;
use crate::discovery::{blast, edges};
use crate::manifest::PackageJson;
use crate::roots::{self, Root};

/// The canonical ecosystem id.
pub(crate) const JS: &str = "js";

/// The name of a repository-root package that declares no `name`.
const UNNAMED_ROOT: &str = "root";

/// The capability prefix recording each package script.
const SCRIPT_CAPABILITY: &str = "script:";

/// One discovered package: its root, directory, manifest, and module.
#[derive(Debug)]
pub(crate) struct Package {
    /// Index of the owning root.
    pub(crate) root: usize,
    /// The repo-relative package directory (empty for the project root).
    pub(crate) dir: PathBuf,
    /// The package manifest.
    pub(crate) manifest: PackageJson,
    /// The module being built.
    pub(crate) module: Module,
}

/// Discover every JavaScript package, workspace, and edge under
/// `request.project_root`.
pub(crate) fn discover(
    config: &JsConfig,
    request: &DiscoverRequest,
) -> AppResult<DiscoverResponse> {
    let ecosystem = EcosystemId::new(JS)?;
    let project_root = request.project_root.as_path();
    let roots = roots::resolve(config, project_root)?;

    let mut response = DiscoverResponse::new(ecosystem.clone());
    response.schema_version = request.schema_version;
    let mut packages: Vec<Package> = Vec::new();
    let mut ids: BTreeMap<ModuleRef, PathBuf> = BTreeMap::new();
    for (index, root) in roots.iter().enumerate() {
        let workspace_id = workspace_id(&root.dir)?;
        let first = packages.len();
        let mut names: BTreeMap<String, PathBuf> = BTreeMap::new();
        for (dir, manifest) in root_packages(project_root, root)? {
            if let Some(name) = &manifest.name
                && let Some(existing) = names.insert(name.clone(), dir.clone())
            {
                return Err(AppError::new(
                    ErrorCode::Conflict,
                    format!(
                        "JavaScript root '{}' has two packages named '{name}': '{}' and '{}'; \
                         package names must be unique within a root",
                        roots::label(&root.dir),
                        roots::manifest_label(&existing),
                        roots::manifest_label(&dir)
                    ),
                ));
            }
            let id = ModuleRef::new(ecosystem.clone(), module_name(&dir, &manifest))?;
            if let Some(existing) = ids.insert(id.clone(), dir.clone()) {
                return Err(AppError::new(
                    ErrorCode::Conflict,
                    format!(
                        "duplicate module '{id}': both '{}' and '{}' resolve to the same name; \
                         module names (the package's repo-relative directory) must be unique",
                        roots::manifest_label(&existing),
                        roots::manifest_label(&dir)
                    ),
                ));
            }
            let mut module = Module::new(id, RepoPath::new(roots::label(&dir))?);
            module.package.clone_from(&manifest.name);
            module.manifest = Some(RepoPath::new(roots::manifest_label(&dir))?);
            module.workspace = Some(workspace_id.clone());
            module.unpublishable = manifest.private;
            record_scripts(&mut module, &manifest, &mut response.warnings);
            packages.push(Package {
                root: index,
                dir,
                manifest,
                module,
            });
        }
        if packages.len() > first {
            let root_path = RepoPath::new(roots::label(&root.dir))?;
            let mut workspace = Workspace::new(
                workspace_id,
                root_path,
                ToolchainTag::new(root.manager.tool().name()),
            );
            workspace.blast_radius = blast::files(&root.dir, root.manager);
            response.workspaces.push(workspace);
        }
    }

    response.edges = edges::link(project_root, &mut packages, &mut response.warnings)?;
    response.modules = packages.into_iter().map(|package| package.module).collect();
    response
        .modules
        .sort_by(|left, right| left.id.cmp(&right.id));
    Ok(response)
}

/// The packages a root contributes: the root package itself when it is a real
/// package (it has a name or its own scripts, or it is not a workspace root),
/// then every member.
///
/// A workspace root's script that a member also declares is an orchestrator
/// (`turbo run test`, `npm test --workspaces`) that fans out to the members
/// Toven already runs, so it is not one of the root package's scripts.
pub(crate) fn root_packages(
    project_root: &Path,
    root: &Root,
) -> AppResult<Vec<(PathBuf, PackageJson)>> {
    let mut members = Vec::with_capacity(root.members.len());
    for member in &root.members {
        members.push((member.clone(), roots::read_manifest(project_root, member)?));
    }
    let mut manifest = root.manifest.clone();
    manifest.scripts.retain(|script, _| {
        !members
            .iter()
            .any(|(_, member)| member.scripts.contains_key(script))
    });
    let mut packages = Vec::with_capacity(members.len() + 1);
    if manifest.name.is_some() || !manifest.scripts.is_empty() || !root.is_workspace() {
        packages.push((root.dir.clone(), manifest));
    }
    packages.extend(members);
    Ok(packages)
}

/// The module name: the repo-relative directory with `/` folded to `-`, or,
/// for the repository-root package, its unscoped package name (`root` when it
/// has none).
fn module_name(dir: &Path, manifest: &PackageJson) -> String {
    if dir.as_os_str().is_empty() {
        return manifest
            .name
            .as_deref()
            .map(|name| name.rsplit('/').next().unwrap_or(name))
            .filter(|name| !name.is_empty())
            .unwrap_or(UNNAMED_ROOT)
            .to_string();
    }
    roots::label(dir).replace('/', "-")
}

/// Record a `script:<name>` capability per script. A script name that is not a
/// valid capability is skipped with a warning.
fn record_scripts(module: &mut Module, manifest: &PackageJson, warnings: &mut Vec<String>) {
    for script in manifest.scripts.keys() {
        match Capability::new(format!("{SCRIPT_CAPABILITY}{script}")) {
            Ok(capability) => {
                module.capabilities.insert(capability);
            }
            Err(error) => warnings.push(format!(
                "module '{}': script '{script}' is not recorded as a capability: {error}",
                module.id
            )),
        }
    }
}

/// The workspace id of a root: `js` for the repository root, `js:<dir>`
/// otherwise.
fn workspace_id(dir: &Path) -> AppResult<WorkspaceId> {
    if dir.as_os_str().is_empty() {
        WorkspaceId::new(JS)
    } else {
        WorkspaceId::new(format!("{JS}:{}", roots::label(dir)))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{module_name, workspace_id};
    use crate::manifest::PackageJson;

    fn named(name: Option<&str>) -> PackageJson {
        let json = name.map_or_else(
            || "{}".to_string(),
            |name| format!(r#"{{"name":"{name}"}}"#),
        );
        rskit_codec::decode(&rskit_codec::JsonCodec::default(), &json).expect("json")
    }

    #[test]
    fn root_package_uses_its_unscoped_name() {
        let root = Path::new("");
        assert_eq!(
            module_name(root, &named(Some("@acme/platform"))),
            "platform"
        );
        assert_eq!(module_name(root, &named(Some("tools"))), "tools");
        assert_eq!(module_name(root, &named(None)), "root");
    }

    #[test]
    fn nested_packages_fold_their_directory() {
        assert_eq!(
            module_name(Path::new("packages/std"), &named(Some("@acme/std"))),
            "packages-std"
        );
        assert_eq!(module_name(Path::new("web"), &named(None)), "web");
    }

    #[test]
    fn workspace_ids_follow_the_root_directory() {
        assert_eq!(workspace_id(Path::new("")).unwrap().as_str(), "js");
        assert_eq!(
            workspace_id(Path::new("gen/ts")).unwrap().as_str(),
            "js:gen/ts"
        );
    }
}
