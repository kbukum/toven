//! Pure affected-module mapping.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use crate::{
    core::{AppResult, Module, ScopedModuleKey, scoped_module_key},
    engine::graph::dependents_closure,
};

/// A changed path relative to the workspace root.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct ChangedPath {
    /// Workspace-relative path.
    pub(crate) path: PathBuf,
}

impl ChangedPath {
    /// Create a changed path.
    #[must_use]
    pub(crate) fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

/// Result of affected-module mapping.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct AffectedModules {
    /// Directly changed modules.
    pub(crate) direct: BTreeSet<ScopedModuleKey>,
    /// Direct modules plus all reverse dependents.
    pub(crate) closure: BTreeSet<ScopedModuleKey>,
    /// Changed paths that did not map to a module and forced all modules affected.
    pub(crate) global_paths: Vec<PathBuf>,
}

/// Map changed workspace paths to modules and expand through reverse dependents.
pub(crate) fn affected_modules(
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
        direct.extend(modules.iter().map(scoped_module_key));
    }

    let closure = dependents_closure(modules, &direct)?;
    Ok(AffectedModules {
        direct,
        closure,
        global_paths,
    })
}

struct ModuleRoot {
    root: PathBuf,
    source_patterns: Vec<String>,
}

fn module_roots(modules: &[Module]) -> BTreeMap<ScopedModuleKey, ModuleRoot> {
    modules
        .iter()
        .map(|module| {
            (
                scoped_module_key(module),
                ModuleRoot {
                    root: normalize_root(&module.root),
                    source_patterns: module.source_patterns.clone(),
                },
            )
        })
        .collect()
}

fn longest_root_match<'a>(
    roots: &'a BTreeMap<ScopedModuleKey, ModuleRoot>,
    path: &Path,
) -> Option<&'a ScopedModuleKey> {
    let path = normalize_root(path);
    roots
        .iter()
        .filter(|(_, module)| path_matches_module(&path, module))
        .max_by_key(|(_, module)| module.root.components().count())
        .map(|(name, _)| name)
}

fn path_matches_module(path: &Path, module: &ModuleRoot) -> bool {
    let root = &module.root;
    if root.as_os_str().is_empty() || root == Path::new(".") {
        return module
            .source_patterns
            .iter()
            .any(|pattern| path_matches_source_pattern(path, pattern));
    }
    path == root || path.starts_with(root)
}

fn path_matches_source_pattern(path: &Path, pattern: &str) -> bool {
    let pattern = Path::new(pattern);
    let Some(prefix) = pattern
        .to_string_lossy()
        .strip_suffix("/**")
        .map(PathBuf::from)
    else {
        return path == normalize_root(pattern);
    };
    path == prefix || path.starts_with(prefix)
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
    use crate::core::{AdapterId, Module, ModuleId, ScopeId, ScopedModuleKey};

    fn module(name: &str, root: &str, dependencies: &[&str]) -> Module {
        Module {
            scope_id: ScopeId::new("rust").unwrap(),
            adapter_id: AdapterId::new("rust").unwrap(),
            name: ModuleId::new(name).unwrap(),
            package: Some(name.to_string()),
            root: PathBuf::from(root),
            manifest: Some(PathBuf::from("Cargo.toml")),
            dependencies: dependencies
                .iter()
                .map(|dependency| ModuleId::new(*dependency).unwrap())
                .collect(),
            source_patterns: Vec::new(),
        }
    }

    fn module_with_sources(
        name: &str,
        root: &str,
        dependencies: &[&str],
        source_patterns: &[&str],
    ) -> Module {
        Module {
            source_patterns: source_patterns
                .iter()
                .map(|pattern| (*pattern).to_string())
                .collect(),
            ..module(name, root, dependencies)
        }
    }

    fn key(scope: &str, module: &str) -> ScopedModuleKey {
        (scope.to_string(), ModuleId::new(module).unwrap())
    }

    #[test]
    fn uses_longest_module_root_match() {
        let modules = [
            module("parent", "crates", &[]),
            module("child", "crates/child", &[]),
        ];

        let affected =
            affected_modules(&modules, &[ChangedPath::new("crates/child/src/lib.rs")]).unwrap();

        assert!(affected.direct.contains(&key("rust", "child")));
        assert!(!affected.direct.contains(&key("rust", "parent")));
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
            [key("rust", "app"), key("rust", "core")]
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

    #[test]
    fn nested_unmatched_paths_fail_closed_even_with_root_module() {
        let modules = [
            module_with_sources("root", ".", &[], &["Cargo.toml", "src/**"]),
            module("app", "crates/app", &["root"]),
            module("util", "crates/util", &[]),
        ];

        let affected =
            affected_modules(&modules, &[ChangedPath::new(".github/workflows/ci.yml")]).unwrap();

        assert_eq!(affected.closure.len(), 3);
        assert_eq!(
            affected.global_paths,
            [PathBuf::from(".github/workflows/ci.yml")]
        );
    }

    #[test]
    fn root_module_source_paths_affect_root_module_not_everything() {
        let modules = [
            module_with_sources("root", ".", &[], &["Cargo.toml", "src/**"]),
            module("app", "crates/app", &["root"]),
            module("util", "crates/util", &[]),
        ];

        let affected = affected_modules(&modules, &[ChangedPath::new("src/lib.rs")]).unwrap();

        assert_eq!(
            affected.closure,
            [key("rust", "app"), key("rust", "root")]
                .into_iter()
                .collect()
        );
        assert!(affected.global_paths.is_empty());
    }

    #[test]
    fn keeps_duplicate_module_names_separate_by_scope() {
        let modules = [
            module("shared", "main/shared", &[]),
            Module {
                scope_id: ScopeId::new("contrib").unwrap(),
                root: PathBuf::from("contrib/shared"),
                ..module("shared", "main/shared", &[])
            },
        ];

        let affected =
            affected_modules(&modules, &[ChangedPath::new("contrib/shared/src/lib.rs")]).unwrap();

        assert_eq!(
            affected.direct,
            [key("contrib", "shared")].into_iter().collect()
        );
    }

    #[test]
    fn expands_unique_cross_scope_dependents() {
        let modules = [
            Module {
                scope_id: ScopeId::new("app").unwrap(),
                dependencies: vec![ModuleId::new("shared").unwrap()],
                ..module("api", "app/api", &[])
            },
            Module {
                scope_id: ScopeId::new("lib").unwrap(),
                ..module("shared", "lib/shared", &[])
            },
        ];

        let affected = affected_modules(&modules, &[ChangedPath::new("lib/shared/src/lib.rs")])
            .expect("cross-scope affected resolves");

        assert_eq!(
            affected.closure,
            [key("app", "api"), key("lib", "shared")]
                .into_iter()
                .collect()
        );
    }
}
