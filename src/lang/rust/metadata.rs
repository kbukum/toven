//! Cargo metadata normalization for Rust discovery.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use cargo_metadata::{DependencyKind, Metadata, MetadataCommand, Package, PackageId};

use crate::core::{AppError, AppResult, Module, ModuleId};

/// Discover Rust modules from a Cargo workspace root.
pub(super) fn discover_modules(workspace_root: impl AsRef<Path>) -> AppResult<Vec<Module>> {
    let metadata = load_metadata(workspace_root.as_ref())?;
    modules_from_metadata(&metadata)
}

fn load_metadata(workspace_root: &Path) -> AppResult<Metadata> {
    let manifest_path = workspace_root.join("Cargo.toml");
    if !manifest_path.is_file() {
        return Err(AppError::invalid_input(
            "workspace.root",
            format!("Cargo.toml not found at {}", manifest_path.display()),
        ));
    }

    MetadataCommand::new()
        .manifest_path(&manifest_path)
        .current_dir(workspace_root)
        .exec()
        .map_err(|error| {
            AppError::invalid_input(
                "workspace.root",
                format!("failed to read cargo metadata: {error}"),
            )
        })
}

fn modules_from_metadata(metadata: &Metadata) -> AppResult<Vec<Module>> {
    let workspace_root = Path::new(metadata.workspace_root.as_str());
    let workspace_ids: BTreeSet<PackageId> = metadata.workspace_members.iter().cloned().collect();
    let packages_by_id: BTreeMap<PackageId, &Package> = metadata
        .packages
        .iter()
        .map(|package| (package.id.clone(), package))
        .collect();
    let mut modules = Vec::with_capacity(workspace_ids.len());

    for package in metadata.workspace_packages() {
        let name = ModuleId::new(package.name.to_string())?;
        let root = package_root(package, workspace_root)?;
        modules.push(Module {
            dependencies: workspace_dependencies(
                metadata,
                &packages_by_id,
                &workspace_ids,
                package,
            )?,
            source_patterns: source_patterns(&root),
            package: Some(package.name.to_string()),
            name,
            root,
        });
    }

    modules.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(modules)
}

fn package_root(package: &Package, workspace_root: &Path) -> AppResult<PathBuf> {
    let manifest_path = Path::new(package.manifest_path.as_str());
    let package_root = manifest_path.parent().ok_or_else(|| {
        AppError::invalid_input(
            "cargo.metadata",
            format!("package '{}' has no manifest parent", package.name),
        )
    })?;
    let relative = package_root.strip_prefix(workspace_root).map_err(|error| {
        AppError::invalid_input(
            "cargo.metadata",
            format!(
                "package '{}' is outside workspace root '{}': {error}",
                package.name,
                workspace_root.display()
            ),
        )
    })?;
    if relative.as_os_str().is_empty() {
        Ok(PathBuf::from("."))
    } else {
        Ok(relative.to_path_buf())
    }
}

fn workspace_dependencies(
    metadata: &Metadata,
    packages_by_id: &BTreeMap<PackageId, &Package>,
    workspace_ids: &BTreeSet<PackageId>,
    package: &Package,
) -> AppResult<Vec<ModuleId>> {
    let Some(resolve) = &metadata.resolve else {
        return Ok(Vec::new());
    };
    let Some(node) = resolve.nodes.iter().find(|node| node.id == package.id) else {
        return Ok(Vec::new());
    };

    let mut dependencies = BTreeSet::new();
    for dependency in &node.deps {
        if !workspace_ids.contains(&dependency.pkg) || is_dev_only_dependency(dependency) {
            continue;
        }
        let dependency_package = packages_by_id.get(&dependency.pkg).ok_or_else(|| {
            AppError::invalid_input(
                "cargo.metadata",
                format!(
                    "resolved package '{}' was not returned by cargo",
                    dependency.pkg
                ),
            )
        })?;
        dependencies.insert(ModuleId::new(dependency_package.name.to_string())?);
    }
    Ok(dependencies.into_iter().collect())
}

fn is_dev_only_dependency(dependency: &cargo_metadata::NodeDep) -> bool {
    !dependency.dep_kinds.is_empty()
        && dependency
            .dep_kinds
            .iter()
            .all(|kind| kind.kind == DependencyKind::Development)
}

fn source_patterns(root: &Path) -> Vec<String> {
    [root.join("Cargo.toml"), root.join("src/**")]
        .into_iter()
        .map(|path| normalize_pattern(&path))
        .collect()
}

fn normalize_pattern(path: &Path) -> String {
    let value = path.to_string_lossy();
    value
        .strip_prefix("./")
        .map_or_else(|| value.to_string(), ToString::to_string)
}
