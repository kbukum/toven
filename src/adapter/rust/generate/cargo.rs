//! Cargo manifest detection for Rust config generation.

use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
};

use crate::core::{AppError, AppResult};

pub(super) const SKIPPED_NESTED_MANIFEST_DIRS: &[&str] = &[
    ".git",
    ".toven",
    "target",
    "node_modules",
    "examples",
    "fuzz",
];

pub(super) fn resolve_manifests(
    root: &Path,
    explicit: &[PathBuf],
) -> AppResult<Option<Vec<PathBuf>>> {
    if explicit.is_empty() {
        let manifest = PathBuf::from("Cargo.toml");
        if root.join(&manifest).is_file() {
            return Ok(Some(vec![manifest]));
        }

        let manifests = discover_nested_workspace_manifests(root)?;
        return Ok((!manifests.is_empty()).then_some(manifests));
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

fn discover_nested_workspace_manifests(root: &Path) -> AppResult<Vec<PathBuf>> {
    let mut manifests = BTreeSet::new();
    let entries = fs::read_dir(root).map_err(|error| {
        AppError::invalid_input(
            "generate.root",
            format!(
                "failed to read root directory '{}': {error}",
                root.display()
            ),
        )
    })?;

    for entry in entries {
        let entry = entry.map_err(|error| {
            AppError::invalid_input(
                "generate.root",
                format!(
                    "failed to read directory entry in '{}': {error}",
                    root.display()
                ),
            )
        })?;
        let entry_type = entry.file_type().map_err(|error| {
            AppError::invalid_input(
                "generate.root",
                format!(
                    "failed to inspect directory entry '{}' in '{}': {error}",
                    entry.file_name().to_string_lossy(),
                    root.display()
                ),
            )
        })?;
        if !entry_type.is_dir() {
            continue;
        }
        if should_skip_nested_manifest_dir(&entry.file_name()) {
            continue;
        }

        let manifest = entry.path().join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }

        let relative = manifest.strip_prefix(root).map_err(|error| {
            AppError::invalid_input(
                "generate.root",
                format!(
                    "manifest '{}' is outside root '{}': {error}",
                    manifest.display(),
                    root.display()
                ),
            )
        })?;
        manifests.insert(normalize_relative_path(relative));
    }

    Ok(manifests.into_iter().collect())
}

fn should_skip_nested_manifest_dir(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| SKIPPED_NESTED_MANIFEST_DIRS.contains(&name))
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

    #[test]
    fn detects_nested_workspace_manifests_when_root_missing() {
        let root = rskit_testutil::test_workspace!("generate-rust-nested-workspaces");
        fs::create_dir_all(root.path().join("core")).expect("create core workspace");
        fs::create_dir_all(root.path().join("contrib")).expect("create contrib workspace");
        fs::create_dir_all(root.path().join("contrib/crates/lib")).expect("create package dir");

        fs::write(root.path().join("core/Cargo.toml"), "[workspace]\n")
            .expect("write core manifest");
        fs::write(root.path().join("contrib/Cargo.toml"), "[workspace]\n")
            .expect("write contrib manifest");
        fs::write(
            root.path().join("contrib/crates/lib/Cargo.toml"),
            "[package]\nname = \"lib\"\nversion = \"0.1.0\"\n",
        )
        .expect("write package manifest");

        let manifests = resolve_manifests(root.path(), &[])
            .expect("resolve succeeds")
            .expect("nested manifests found");

        assert_eq!(
            manifests,
            [
                PathBuf::from("contrib/Cargo.toml"),
                PathBuf::from("core/Cargo.toml")
            ]
        );
    }

    #[test]
    fn explicit_manifests_override_default_nested_discovery() {
        let root = rskit_testutil::test_workspace!("generate-rust-explicit-override");
        fs::create_dir_all(root.path().join("core")).expect("create core workspace");
        fs::create_dir_all(root.path().join("contrib")).expect("create contrib workspace");
        fs::write(root.path().join("core/Cargo.toml"), "[workspace]\n")
            .expect("write core manifest");
        fs::write(root.path().join("contrib/Cargo.toml"), "[workspace]\n")
            .expect("write contrib manifest");

        let manifests = resolve_manifests(root.path(), &[PathBuf::from("core/Cargo.toml")])
            .expect("resolve succeeds")
            .expect("explicit manifest found");

        assert_eq!(manifests, [PathBuf::from("core/Cargo.toml")]);
    }

    #[test]
    fn ignores_non_production_top_level_manifest_dirs() {
        let root = rskit_testutil::test_workspace!("generate-rust-skip-non-production");
        fs::create_dir_all(root.path().join("core")).expect("create core workspace");
        fs::create_dir_all(root.path().join("examples")).expect("create examples workspace");
        fs::create_dir_all(root.path().join("fuzz")).expect("create fuzz workspace");
        fs::write(root.path().join("core/Cargo.toml"), "[workspace]\n")
            .expect("write core manifest");
        fs::write(root.path().join("examples/Cargo.toml"), "[workspace]\n")
            .expect("write examples manifest");
        fs::write(
            root.path().join("fuzz/Cargo.toml"),
            "[package]\nname = \"fuzz\"\nversion = \"0.0.0\"\n",
        )
        .expect("write fuzz manifest");

        let manifests = resolve_manifests(root.path(), &[])
            .expect("resolve succeeds")
            .expect("nested manifests found");

        assert_eq!(manifests, [PathBuf::from("core/Cargo.toml")]);
    }
}
