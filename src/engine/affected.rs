//! Pure affected-module mapping.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use crate::{
    core::{AppResult, Module, ModuleId},
    engine::graph::dependents_closure,
};

/// A changed path relative to the workspace root.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ChangedPath {
    /// Workspace-relative path.
    pub path: PathBuf,
}

impl ChangedPath {
    /// Create a changed path.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

/// Result of affected-module mapping.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AffectedModules {
    /// Directly changed modules.
    pub direct: BTreeSet<ModuleId>,
    /// Direct modules plus all reverse dependents.
    pub closure: BTreeSet<ModuleId>,
    /// Changed paths that did not map to a module and forced all modules affected.
    pub global_paths: Vec<PathBuf>,
}

/// Map changed workspace paths to modules and expand through reverse dependents.
pub fn affected_modules(
    modules: &[Module],
    changed_paths: &[ChangedPath],
) -> AppResult<AffectedModules> {
    if changed_paths.is_empty() {
        return Ok(AffectedModules {
            direct: BTreeSet::new(),
            closure: BTreeSet::new(),
            global_paths: Vec::new(),
        });
    }

    let roots = module_roots(modules);
    let mut direct = BTreeSet::new();
    let mut global_paths = Vec::new();

    for changed in changed_paths {
        if is_workspace_root_file(&changed.path) {
            global_paths.push(changed.path.clone());
            continue;
        }

        match longest_root_match(&roots, &changed.path) {
            Some(module) => {
                direct.insert(module.clone());
            }
            None => {
                global_paths.push(changed.path.clone());
            }
        }
    }

    if !global_paths.is_empty() {
        direct.extend(modules.iter().map(|module| module.name.clone()));
    }

    let closure = dependents_closure(modules, &direct)?;
    Ok(AffectedModules {
        direct,
        closure,
        global_paths,
    })
}

fn module_roots(modules: &[Module]) -> BTreeMap<ModuleId, PathBuf> {
    modules
        .iter()
        .map(|module| (module.name.clone(), normalize_root(&module.root)))
        .collect()
}

fn longest_root_match<'a>(
    roots: &'a BTreeMap<ModuleId, PathBuf>,
    path: &Path,
) -> Option<&'a ModuleId> {
    let path = normalize_root(path);
    roots
        .iter()
        .filter(|(_, root)| path_matches_root(&path, root))
        .max_by_key(|(_, root)| root.components().count())
        .map(|(name, _)| name)
}

fn path_matches_root(path: &Path, root: &Path) -> bool {
    root.as_os_str().is_empty() || root == Path::new(".") || path == root || path.starts_with(root)
}

fn is_workspace_root_file(path: &Path) -> bool {
    path.components().count() == 1
}

fn normalize_root(path: &Path) -> PathBuf {
    if path.as_os_str().is_empty() {
        return PathBuf::from(".");
    }
    path.components().collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{ChangedPath, affected_modules};
    use crate::core::{Module, ModuleId};

    fn module(name: &str, root: &str, dependencies: &[&str]) -> Module {
        Module {
            name: ModuleId::new(name).unwrap(),
            package: Some(name.to_string()),
            root: PathBuf::from(root),
            dependencies: dependencies
                .iter()
                .map(|dependency| ModuleId::new(*dependency).unwrap())
                .collect(),
            source_patterns: Vec::new(),
        }
    }

    #[test]
    fn uses_longest_module_root_match() {
        let modules = [
            module("parent", "crates", &[]),
            module("child", "crates/child", &[]),
        ];

        let affected =
            affected_modules(&modules, &[ChangedPath::new("crates/child/src/lib.rs")]).unwrap();

        assert!(affected.direct.contains(&ModuleId::new("child").unwrap()));
        assert!(!affected.direct.contains(&ModuleId::new("parent").unwrap()));
    }

    #[test]
    fn expands_changed_dependency_to_dependents() {
        let modules = [
            module("app", "app", &["core"]),
            module("core", "core", &[]),
            module("docs", "docs", &[]),
        ];

        let affected = affected_modules(&modules, &[ChangedPath::new("core/src/lib.rs")]).unwrap();

        assert_eq!(
            affected.closure,
            [
                ModuleId::new("app").unwrap(),
                ModuleId::new("core").unwrap()
            ]
            .into_iter()
            .collect()
        );
    }

    #[test]
    fn unmatched_paths_fail_closed_to_all_modules() {
        let modules = [module("app", "app", &[]), module("core", "core", &[])];

        let affected = affected_modules(&modules, &[ChangedPath::new("Cargo.lock")]).unwrap();

        assert_eq!(affected.closure.len(), 2);
        assert_eq!(affected.global_paths, [PathBuf::from("Cargo.lock")]);
    }

    #[test]
    fn workspace_root_files_affect_all_modules_even_with_root_module() {
        let modules = [
            module("root", ".", &[]),
            module("app", "crates/app", &["root"]),
            module("util", "crates/util", &[]),
        ];

        let affected = affected_modules(&modules, &[ChangedPath::new("Cargo.lock")]).unwrap();

        assert_eq!(affected.closure.len(), 3);
        assert_eq!(affected.global_paths, [PathBuf::from("Cargo.lock")]);
    }
}
