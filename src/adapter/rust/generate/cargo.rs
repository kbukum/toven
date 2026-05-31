//! Cargo manifest detection for Rust config generation.

use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
};

use crate::core::{AppError, AppResult};

pub(super) fn resolve_manifests(
    root: &Path,
    explicit: &[PathBuf],
) -> AppResult<Option<Vec<PathBuf>>> {
    if explicit.is_empty() {
        let manifest = PathBuf::from("Cargo.toml");
        return Ok(root.join(&manifest).is_file().then_some(vec![manifest]));
    }

    let mut manifests = BTreeSet::new();
    for manifest in explicit {
        let manifest = normalize_manifest(root, manifest)?;
        let manifest_path = root.join(&manifest);
        if !manifest_path.is_file() {
            return Err(AppError::invalid_input(
                "generate.manifest",
                format!("manifest not found at {}", manifest_path.display()),
            ));
        }
        manifests.insert(manifest);
    }

    Ok(Some(manifests.into_iter().collect()))
}

fn normalize_manifest(root: &Path, manifest: &Path) -> AppResult<PathBuf> {
    let relative = if manifest.is_absolute() {
        manifest.strip_prefix(root).map_err(|error| {
            AppError::invalid_input(
                "generate.manifest",
                format!(
                    "absolute manifest '{}' must be under root '{}': {error}",
                    manifest.display(),
                    root.display()
                ),
            )
        })?
    } else {
        manifest
    };
    validate_relative_manifest(relative)?;
    Ok(normalize_relative_path(relative))
}

fn validate_relative_manifest(path: &Path) -> AppResult<()> {
    if path.as_os_str().is_empty() {
        return Err(AppError::invalid_input(
            "generate.manifest",
            "manifest path cannot be empty",
        ));
    }
    for component in path.components() {
        match component {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(AppError::invalid_input(
                    "generate.manifest",
                    "manifest path must stay inside the selected root",
                ));
            }
            Component::CurDir | Component::Normal(_) => {}
        }
    }
    if path.file_name().and_then(|name| name.to_str()) != Some("Cargo.toml") {
        return Err(AppError::invalid_input(
            "generate.manifest",
            "Rust manifests must be Cargo.toml files",
        ));
    }
    Ok(())
}

fn normalize_relative_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => normalized.push(value),
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use super::resolve_manifests;

    #[test]
    fn detects_root_cargo_manifest() {
        let root = rskit_testutil::test_workspace!("generate-rust-detect");
        fs::write(root.path().join("Cargo.toml"), "[workspace]\n").expect("write manifest");

        let manifests = resolve_manifests(root.path(), &[])
            .expect("resolve succeeds")
            .expect("manifest found");

        assert_eq!(manifests, [PathBuf::from("Cargo.toml")]);
    }

    #[test]
    fn rejects_parent_manifest_paths() {
        let root = rskit_testutil::test_workspace!("generate-rust-parent");

        let error = resolve_manifests(root.path(), &[PathBuf::from("../Cargo.toml")])
            .expect_err("parent path fails");

        assert!(error.message.contains("inside the selected root"));
    }
}
