//! Cargo-metadata discovery: run `cargo metadata` per configured manifest,
//! parse the result, and fold it into the unified `{workspaces, modules,
//! edges}` [`DiscoverResponse`].
//!
//! `cargo metadata` is invoked through `rskit-process` (captured, bounded,
//! timed-out — never a shell string) and the JSON is parsed with the
//! `cargo_metadata` types. Cross-manifest path dependencies become intra-
//! ecosystem [`Edge`]s, so a Rust project spanning several Cargo workspaces
//! surfaces as one module + edge set.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::safe_join;
use rskit_process::{CapturedIo, ProcessConfig, ProcessIo, ProcessSpec, run};
use toven_model::{
    DepKind, EcosystemId, Edge, Module, ModuleRef, RepoPath, ToolchainTag, Workspace, WorkspaceId,
};
use toven_ports::{DiscoverRequest, DiscoverResponse};

use crate::config::RustConfig;
use crate::discovery::blast;
use crate::manifests;

/// Hard bound on retained `cargo metadata` output (16 MiB). Large enough for
/// big polyglot workspaces, bounded so a runaway process cannot exhaust memory.
const MAX_METADATA_BYTES: usize = 16 * 1024 * 1024;

/// Wall-clock bound on a single `cargo metadata` invocation.
const METADATA_TIMEOUT: Duration = Duration::from_mins(2);

/// The cargo driver name stamped on every discovered [`Workspace`].
const CARGO_TOOL: &str = "cargo";

/// Discover all Rust modules, workspaces, and edges under
/// `request.project_root`.
pub(crate) fn discover(
    config: &RustConfig,
    request: &DiscoverRequest,
) -> AppResult<DiscoverResponse> {
    let ecosystem = rust_id()?;
    let project_root = request.project_root.as_path();

    let mut workspaces: BTreeMap<WorkspaceId, Workspace> = BTreeMap::new();
    let mut modules: BTreeMap<ModuleRef, Module> = BTreeMap::new();
    let mut path_deps: Vec<(String, String, DepKind)> = Vec::new();

    for manifest in manifests::resolve(config, project_root)? {
        let metadata = run_metadata(project_root, &manifest)?;
        fold_metadata(
            &ecosystem,
            project_root,
            &metadata,
            &mut workspaces,
            &mut modules,
            &mut path_deps,
        )?;
    }

    let edges = build_edges(&ecosystem, &modules, &path_deps)?;

    let mut response = DiscoverResponse::new(ecosystem);
    response.schema_version = request.schema_version;
    response.workspaces = workspaces.into_values().collect();
    response.modules = modules.into_values().collect();
    response.edges = edges;
    Ok(response)
}

/// The canonical Rust ecosystem id.
fn rust_id() -> AppResult<EcosystemId> {
    EcosystemId::new("rust")
}

/// Run `cargo metadata --no-deps` for one manifest and parse its JSON output.
fn run_metadata(project_root: &Path, manifest: &str) -> AppResult<cargo_metadata::Metadata> {
    let manifest_abs = safe_join(project_root, Path::new(manifest)).map_err(|error| {
        AppError::invalid_input(
            "ecosystems.rust.manifests",
            format!("manifest '{manifest}' escapes the project root: {error}"),
        )
    })?;

    let spec = ProcessSpec::new(CARGO_TOOL)
        .arg("metadata")
        .arg("--no-deps")
        .arg("--format-version")
        .arg("1")
        .arg("--manifest-path")
        .arg(&manifest_abs)
        .dir(project_root);
    let config = ProcessConfig::default()
        .with_io(ProcessIo::captured(CapturedIo::new()))
        .with_timeout(Some(METADATA_TIMEOUT))
        .with_max_output_bytes(MAX_METADATA_BYTES);

    let result = run(&spec, &config)?;
    if result.timed_out {
        return Err(AppError::new(
            ErrorCode::Timeout,
            format!("`cargo metadata` for '{manifest}' timed out"),
        ));
    }
    if result.stdout_truncated || result.stderr_truncated {
        return Err(AppError::new(
            ErrorCode::Internal,
            format!("`cargo metadata` output for '{manifest}' exceeded {MAX_METADATA_BYTES} bytes"),
        ));
    }
    if !result.success() {
        return Err(AppError::new(
            ErrorCode::Internal,
            format!(
                "`cargo metadata` for '{manifest}' failed (exit {:?}): {}",
                result.exit_code,
                result.stderr.trim()
            ),
        ));
    }

    rskit_codec::decode::<cargo_metadata::Metadata>(
        &rskit_codec::JsonCodec::default(),
        &result.stdout,
    )
    .map_err(|error| {
        AppError::new(
            ErrorCode::InvalidFormat,
            format!("failed to parse `cargo metadata` output for '{manifest}'"),
        )
        .with_cause(error)
    })
}

/// Fold one workspace's metadata into the running module/workspace/edge sets.
fn fold_metadata(
    ecosystem: &EcosystemId,
    project_root: &Path,
    metadata: &cargo_metadata::Metadata,
    workspaces: &mut BTreeMap<WorkspaceId, Workspace>,
    modules: &mut BTreeMap<ModuleRef, Module>,
    path_deps: &mut Vec<(String, String, DepKind)>,
) -> AppResult<()> {
    let workspace_root = repo_relative(project_root, metadata.workspace_root.as_std_path())?;
    let workspace_id = workspace_id(&workspace_root)?;

    let mut workspace = Workspace::new(
        workspace_id.clone(),
        workspace_root.clone(),
        ToolchainTag::new(CARGO_TOOL),
    );
    blast::annotate_workspace(&mut workspace, &workspace_root);
    workspaces.entry(workspace_id.clone()).or_insert(workspace);

    let members: BTreeSet<&cargo_metadata::PackageId> = metadata.workspace_members.iter().collect();

    for package in &metadata.packages {
        if !members.contains(&package.id) {
            continue;
        }
        let name = package.name.to_string();
        let manifest = repo_relative(project_root, package.manifest_path.as_std_path())?;
        let root = manifest_parent(&manifest)?;
        let id = ModuleRef::new(ecosystem.clone(), &name)?;

        if let Some(existing) = modules.get(&id) {
            return Err(AppError::new(
                ErrorCode::Conflict,
                format!(
                    "duplicate module '{id}': package '{name}' is declared by both manifest \
                     '{}' and '{}'; package names must be unique across the project",
                    existing
                        .manifest
                        .as_ref()
                        .map_or_else(|| existing.root.to_string(), ToString::to_string),
                    manifest
                ),
            ));
        }

        let mut module = Module::new(id.clone(), root.clone());
        module.package = Some(name.clone());
        module.manifest = Some(manifest);
        module.workspace = Some(workspace_id.clone());
        module.runnable = has_runnable_target(package);
        blast::annotate_module(&mut module, &workspace_root);
        modules.insert(id, module);

        for dependency in &package.dependencies {
            if dependency.path.is_some() {
                path_deps.push((
                    name.clone(),
                    dependency.name.clone(),
                    dep_kind(dependency.kind),
                ));
            }
        }
    }
    Ok(())
}

/// Turn the collected path dependencies into intra-ecosystem edges, keeping
/// only those whose target resolves to a discovered module (covers within- and
/// cross-workspace path deps; ignores path deps to crates outside the project).
fn build_edges(
    ecosystem: &EcosystemId,
    modules: &BTreeMap<ModuleRef, Module>,
    path_deps: &[(String, String, DepKind)],
) -> AppResult<Vec<Edge>> {
    let mut edges: BTreeSet<Edge> = BTreeSet::new();
    for (from_name, to_name, kind) in path_deps {
        let from = ModuleRef::new(ecosystem.clone(), from_name)?;
        let to = ModuleRef::new(ecosystem.clone(), to_name)?;
        if from != to && modules.contains_key(&to) && modules.contains_key(&from) {
            edges.insert(Edge::new(from, to, *kind));
        }
    }
    Ok(edges.into_iter().collect())
}

/// Whether a package exposes a `bin` target `cargo run` can launch by default.
/// The `run` task argv is `cargo run … -p {package}` with no `--example`, so an
/// example target is not launchable this way; library- and example-only crates
/// have no default runnable, and the scheduler drops a persistent `run` unit
/// against them.
fn has_runnable_target(package: &cargo_metadata::Package) -> bool {
    package.targets.iter().any(cargo_metadata::Target::is_bin)
}

/// Map a cargo dependency kind onto the model's [`DepKind`].
const fn dep_kind(kind: cargo_metadata::DependencyKind) -> DepKind {
    match kind {
        cargo_metadata::DependencyKind::Development => DepKind::Dev,
        cargo_metadata::DependencyKind::Build => DepKind::Build,
        _ => DepKind::Normal,
    }
}

/// Strip `project_root` from an absolute discovery path, yielding a confined
/// repo-relative [`RepoPath`]. Paths outside the project root are rejected.
fn repo_relative(project_root: &Path, absolute: &Path) -> AppResult<RepoPath> {
    let relative = absolute.strip_prefix(project_root).map_err(|_| {
        AppError::invalid_input(
            "discovery.path",
            format!(
                "path '{}' is outside the project root '{}'",
                absolute.display(),
                project_root.display()
            ),
        )
    })?;
    if relative.as_os_str().is_empty() {
        RepoPath::new(".")
    } else {
        RepoPath::new(relative)
    }
}

/// The repo-relative directory containing a manifest (the module root).
fn manifest_parent(manifest: &RepoPath) -> AppResult<RepoPath> {
    match manifest.as_path().parent() {
        Some(parent) if !parent.as_os_str().is_empty() => RepoPath::new(parent),
        _ => RepoPath::new("."),
    }
}

/// Derive a stable [`WorkspaceId`] from a workspace's repo-relative root.
fn workspace_id(root: &RepoPath) -> AppResult<WorkspaceId> {
    let label = root.as_path().to_string_lossy();
    let id = if label == "." {
        "rust".to_string()
    } else {
        format!("rust:{}", PathBuf::from(label.as_ref()).display())
    };
    WorkspaceId::new(id)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use toven_model::{DepKind, RepoPath};

    use super::{dep_kind, manifest_parent, repo_relative, workspace_id};

    #[test]
    fn repo_relative_strips_project_root() {
        let root = Path::new("/repo");
        let resolved = repo_relative(root, Path::new("/repo/crates/app/Cargo.toml")).expect("rel");
        assert_eq!(resolved, RepoPath::new("crates/app/Cargo.toml").unwrap());
    }

    #[test]
    fn repo_relative_maps_root_to_dot() {
        let resolved = repo_relative(Path::new("/repo"), Path::new("/repo")).expect("root");
        assert_eq!(resolved, RepoPath::new(".").unwrap());
    }

    #[test]
    fn repo_relative_rejects_paths_outside_root() {
        assert!(repo_relative(Path::new("/repo"), Path::new("/other/x")).is_err());
    }

    #[test]
    fn manifest_parent_is_the_module_root() {
        let manifest = RepoPath::new("crates/app/Cargo.toml").unwrap();
        assert_eq!(
            manifest_parent(&manifest).unwrap(),
            RepoPath::new("crates/app").unwrap()
        );
    }

    #[test]
    fn root_manifest_parent_is_dot() {
        let manifest = RepoPath::new("Cargo.toml").unwrap();
        assert_eq!(
            manifest_parent(&manifest).unwrap(),
            RepoPath::new(".").unwrap()
        );
    }

    #[test]
    fn workspace_id_uses_root_label() {
        assert_eq!(
            workspace_id(&RepoPath::new(".").unwrap()).unwrap().as_str(),
            "rust"
        );
        assert_eq!(
            workspace_id(&RepoPath::new("contrib").unwrap())
                .unwrap()
                .as_str(),
            "rust:contrib"
        );
    }

    #[test]
    fn dependency_kinds_map_onto_dep_kind() {
        assert_eq!(
            dep_kind(cargo_metadata::DependencyKind::Development),
            DepKind::Dev
        );
        assert_eq!(
            dep_kind(cargo_metadata::DependencyKind::Build),
            DepKind::Build
        );
        assert_eq!(
            dep_kind(cargo_metadata::DependencyKind::Normal),
            DepKind::Normal
        );
    }
}
