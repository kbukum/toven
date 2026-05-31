//! Cargo metadata normalization for Rust discovery.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use cargo_metadata::{DependencyKind, Metadata, MetadataCommand, Node, Package, PackageId};

use crate::core::{AppError, AppResult, Module, ModuleId};

/// Discover Rust modules from Cargo manifests.
pub(in crate::adapter::rust) fn discover_modules(
    project_root: impl AsRef<Path>,
    manifests: &[PathBuf],
) -> AppResult<Vec<Module>> {
    let mut modules_by_name = BTreeMap::new();
    for manifest in manifests {
        let metadata = load_metadata(project_root.as_ref(), manifest)?;
        for module in modules_from_metadata(&metadata, manifest)? {
            if let Some(previous) = modules_by_name.insert(module.name.clone(), module.clone()) {
                return Err(AppError::invalid_input(
                    "profiles.<profile>.manifests",
                    format!(
                        "duplicate Rust package '{}' discovered from '{}' and '{}'",
                        module.name,
                        previous.manifest.as_deref().map_or_else(
                            || "<unknown>".to_string(),
                            |path| path.display().to_string()
                        ),
                        manifest.display()
                    ),
                ));
            }
        }
    }
    Ok(modules_by_name.into_values().collect())
}

fn load_metadata(project_root: &Path, manifest: &Path) -> AppResult<Metadata> {
    let manifest_path = project_root.join(manifest);
    if !manifest_path.is_file() {
        return Err(AppError::invalid_input(
            "profiles.<profile>.manifests",
            format!("Cargo.toml not found at {}", manifest_path.display()),
        ));
    }

    MetadataCommand::new()
        .manifest_path(&manifest_path)
        .current_dir(project_root)
        .exec()
        .map_err(|error| {
            AppError::invalid_input(
                "profiles.<profile>.manifests",
                format!("failed to read cargo metadata: {error}"),
            )
        })
}

fn modules_from_metadata(metadata: &Metadata, manifest: &Path) -> AppResult<Vec<Module>> {
    let workspace_root = Path::new(metadata.workspace_root.as_str());
    let workspace_ids: BTreeSet<PackageId> = metadata.workspace_members.iter().cloned().collect();
    let packages_by_id: BTreeMap<PackageId, &Package> = metadata
        .packages
        .iter()
        .map(|package| (package.id.clone(), package))
        .collect();
    let nodes_by_id: BTreeMap<PackageId, &Node> =
        metadata
            .resolve
            .as_ref()
            .map_or_else(BTreeMap::new, |resolve| {
                resolve
                    .nodes
                    .iter()
                    .map(|node| (node.id.clone(), node))
                    .collect()
            });
    let mut modules = Vec::with_capacity(workspace_ids.len());

    for package in metadata.workspace_packages() {
        let name = ModuleId::new(package.name.to_string())?;
        let root = project_relative_package_root(package, workspace_root, manifest)?;
        modules.push(Module {
            dependencies: workspace_dependencies(
                &nodes_by_id,
                &packages_by_id,
                &workspace_ids,
                package,
            )?,
            source_patterns: source_patterns(&root),
            package: Some(package.name.to_string()),
            name,
            root,
            manifest: Some(manifest.to_path_buf()),
        });
    }

    modules.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(modules)
}

fn project_relative_package_root(
    package: &Package,
    workspace_root: &Path,
    manifest: &Path,
) -> AppResult<PathBuf> {
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
    let manifest_parent = manifest.parent().unwrap_or_else(|| Path::new("."));
    Ok(normalize_project_path(&manifest_parent.join(relative)))
}

fn workspace_dependencies(
    nodes_by_id: &BTreeMap<PackageId, &Node>,
    packages_by_id: &BTreeMap<PackageId, &Package>,
    workspace_ids: &BTreeSet<PackageId>,
    package: &Package,
) -> AppResult<Vec<ModuleId>> {
    let Some(node) = nodes_by_id.get(&package.id) else {
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

fn normalize_project_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(value) => normalized.push(value),
            _ => normalized.push(component.as_os_str()),
        }
    }
    if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}

fn normalize_pattern(path: &Path) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    value
        .strip_prefix("./")
        .map_or_else(|| value.clone(), ToString::to_string)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{normalize_pattern, normalize_project_path};

    #[test]
    fn normalizes_dot_prefixed_patterns() {
        assert_eq!(normalize_pattern(Path::new("./src/**")), "src/**");
        assert_eq!(normalize_pattern(Path::new(".\\src\\**")), "src/**");
    }

    #[test]
    fn normalizes_glob_separators() {
        assert_eq!(
            normalize_pattern(Path::new("crates\\app\\src\\**")),
            "crates/app/src/**"
        );
    }

    #[test]
    fn normalizes_project_paths() {
        assert_eq!(
            normalize_project_path(Path::new("./core/./crates/app")),
            PathBuf::from("core/crates/app")
        );
    }
}
