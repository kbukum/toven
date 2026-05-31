//! Module dependency graph validation.
#![allow(clippy::redundant_pub_crate)]

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    core::{
        AppError, AppResult, DependencyOverlay, Module, ModuleId, ScopedModuleKey,
        scoped_module_display, scoped_module_key,
    },
    engine::overlays::apply_dependency_overlays,
};

/// Provenance for a resolved dependency edge.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum DependencyOrigin {
    /// Edge reported by adapter discovery.
    Inferred,
    /// Edge configured as a project dependency overlay.
    Overlay,
}

/// Canonical scope-qualified dependency graph.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct ResolvedDependencyGraph {
    dependencies: BTreeMap<ScopedModuleKey, BTreeSet<ScopedModuleKey>>,
    dependents: BTreeMap<ScopedModuleKey, Vec<ScopedModuleKey>>,
    origins: BTreeMap<(ScopedModuleKey, ScopedModuleKey), DependencyOrigin>,
}

impl ResolvedDependencyGraph {
    /// Dependencies for `module`.
    pub(crate) fn dependencies(&self, module: &ScopedModuleKey) -> BTreeSet<ScopedModuleKey> {
        self.dependencies.get(module).cloned().unwrap_or_default()
    }

    /// Dependents of `module`.
    pub(crate) fn dependents(&self, module: &ScopedModuleKey) -> &[ScopedModuleKey] {
        self.dependents.get(module).map_or(&[], Vec::as_slice)
    }

    /// Origin of a resolved edge.
    pub(crate) fn origin(
        &self,
        from: &ScopedModuleKey,
        to: &ScopedModuleKey,
    ) -> Option<DependencyOrigin> {
        self.origins.get(&(from.clone(), to.clone())).copied()
    }
}

pub(super) fn dependents_closure(
    modules: &[Module],
    seeds: &BTreeSet<ScopedModuleKey>,
    overlays: &[DependencyOverlay],
) -> AppResult<BTreeSet<ScopedModuleKey>> {
    validate_modules(modules, overlays)?;

    let module_keys = modules
        .iter()
        .map(scoped_module_key)
        .collect::<BTreeSet<_>>();
    let unknown = seeds
        .iter()
        .filter(|seed| !module_keys.contains(*seed))
        .map(scoped_module_display)
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        return Err(AppError::invalid_input(
            "affected",
            format!(
                "affected seed references unknown module(s): {}",
                unknown.join(", ")
            ),
        ));
    }

    let graph = resolve_dependency_graph(modules, overlays)?;
    let mut affected = seeds.clone();
    let mut pending = seeds.iter().cloned().collect::<Vec<_>>();

    while let Some(current) = pending.pop() {
        for next in graph.dependents(&current) {
            if affected.insert(next.clone()) {
                pending.push(next.clone());
            }
        }
    }

    Ok(affected)
}

pub(super) fn validate_modules(
    modules: &[Module],
    overlays: &[DependencyOverlay],
) -> AppResult<()> {
    resolve_dependency_graph(modules, overlays).map(|_| ())
}

/// Resolve all adapter-inferred dependencies plus project overlays.
pub(crate) fn resolve_dependency_graph(
    modules: &[Module],
    overlays: &[DependencyOverlay],
) -> AppResult<ResolvedDependencyGraph> {
    resolve_dependency_graph_with_mode(modules, overlays, MissingDependencyMode::Error)
}

/// Resolve dependencies for a selected subset, ignoring edges outside the subset.
pub(crate) fn resolve_selected_dependency_graph(
    modules: &[Module],
    overlays: &[DependencyOverlay],
) -> AppResult<ResolvedDependencyGraph> {
    resolve_dependency_graph_with_mode(modules, overlays, MissingDependencyMode::Ignore)
}

fn resolve_dependency_graph_with_mode(
    modules: &[Module],
    overlays: &[DependencyOverlay],
    missing_mode: MissingDependencyMode,
) -> AppResult<ResolvedDependencyGraph> {
    let mut keys = BTreeSet::new();
    for module in modules {
        let key = scoped_module_key(module);
        if !keys.insert(key.clone()) {
            return Err(AppError::invalid_input(
                "modules",
                format!("duplicate module '{}'", scoped_module_display(&key)),
            ));
        }
    }

    let selected_by_name = selected_modules_by_name(keys.iter());
    let mut dependencies = BTreeMap::<ScopedModuleKey, BTreeSet<ScopedModuleKey>>::new();
    let mut origins = BTreeMap::<(ScopedModuleKey, ScopedModuleKey), DependencyOrigin>::new();

    for module in modules {
        let module_key = scoped_module_key(module);
        let mut seen = BTreeSet::new();
        for dependency in &module.dependencies {
            if !seen.insert(dependency) {
                return Err(AppError::invalid_input(
                    "modules",
                    format!(
                        "module '{}' has duplicate dependency '{}'",
                        module.name, dependency
                    ),
                ));
            }
            if let Some(dependency_key) = dependency_key_for(module, dependency, &selected_by_name)?
            {
                dependencies
                    .entry(module_key.clone())
                    .or_default()
                    .insert(dependency_key.clone());
                origins.insert(
                    (module_key.clone(), dependency_key),
                    DependencyOrigin::Inferred,
                );
            } else if missing_mode == MissingDependencyMode::Error {
                return Err(AppError::invalid_input(
                    "modules",
                    format!(
                        "module '{}' depends on unknown module '{}'",
                        module.name, dependency
                    ),
                ));
            }
        }
    }

    apply_dependency_overlays(
        &keys,
        overlays,
        missing_mode == MissingDependencyMode::Ignore,
        &mut dependencies,
        &mut origins,
    )?;

    let mut dependents = BTreeMap::<ScopedModuleKey, Vec<ScopedModuleKey>>::new();
    for (from, module_dependencies) in &dependencies {
        for dependency in module_dependencies {
            dependents
                .entry(dependency.clone())
                .or_default()
                .push(from.clone());
        }
    }
    for values in dependents.values_mut() {
        values.sort();
        values.dedup();
    }

    Ok(ResolvedDependencyGraph {
        dependencies,
        dependents,
        origins,
    })
}

pub(crate) fn selected_modules_by_name<'a>(
    keys: impl Iterator<Item = &'a ScopedModuleKey>,
) -> BTreeMap<ModuleId, Vec<ScopedModuleKey>> {
    let mut selected = BTreeMap::<ModuleId, Vec<ScopedModuleKey>>::new();
    for key in keys {
        selected.entry(key.1.clone()).or_default().push(key.clone());
    }
    selected
}

pub(crate) fn dependency_key_for(
    module: &Module,
    dependency: &ModuleId,
    selected_by_name: &BTreeMap<ModuleId, Vec<ScopedModuleKey>>,
) -> AppResult<Option<ScopedModuleKey>> {
    dependency_key_for_scope(module.scope_id.as_str(), dependency, selected_by_name)
}

pub(crate) fn dependency_key_for_scope(
    scope_id: &str,
    dependency: &ModuleId,
    selected_by_name: &BTreeMap<ModuleId, Vec<ScopedModuleKey>>,
) -> AppResult<Option<ScopedModuleKey>> {
    let Some(candidates) = selected_by_name.get(dependency) else {
        return Ok(None);
    };
    if let Some(candidate) = candidates
        .iter()
        .find(|(candidate_scope_id, _)| candidate_scope_id == scope_id)
    {
        return Ok(Some(candidate.clone()));
    }
    if candidates.len() == 1 {
        return Ok(Some(candidates[0].clone()));
    }
    Err(AppError::invalid_input(
        "modules",
        format!(
            "dependency '{dependency}' from scope '{scope_id}' is ambiguous across scopes; add an explicit dependency overlay"
        ),
    ))
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum MissingDependencyMode {
    Error,
    Ignore,
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, path::PathBuf};

    use crate::{
        core::{AdapterId, DependencyOverlay, Module, ModuleId, ScopeId, ScopedModuleKey},
        engine::graph::dependents_closure,
    };

    fn module(name: &str, dependencies: &[&str]) -> Module {
        Module {
            scope_id: ScopeId::new("rust").expect("scope id"),
            adapter_id: AdapterId::new("rust").expect("adapter id"),
            name: ModuleId::new(name).expect("module id"),
            package: Some(name.to_string()),
            root: PathBuf::from(name),
            manifest: Some(PathBuf::from("Cargo.toml")),
            dependencies: dependencies
                .iter()
                .map(|dependency| ModuleId::new(*dependency).expect("module id"))
                .collect(),
            source_patterns: Vec::new(),
        }
    }

    fn scoped_module(scope: &str, name: &str, dependencies: &[&str]) -> Module {
        Module {
            scope_id: ScopeId::new(scope).expect("scope id"),
            ..module(name, dependencies)
        }
    }

    fn key(scope: &str, module: &str) -> ScopedModuleKey {
        (scope.to_string(), ModuleId::new(module).unwrap())
    }

    #[test]
    fn dependents_closure_walks_reverse_edges_only() {
        let modules = [module("a", &["b"]), module("b", &["c"]), module("c", &[])];

        let affected =
            dependents_closure(&modules, &BTreeSet::from([key("rust", "c")]), &[]).unwrap();
        assert_eq!(
            affected,
            BTreeSet::from([key("rust", "a"), key("rust", "b"), key("rust", "c")])
        );

        let affected =
            dependents_closure(&modules, &BTreeSet::from([key("rust", "a")]), &[]).unwrap();
        assert_eq!(affected, BTreeSet::from([key("rust", "a")]));
    }

    #[test]
    fn dependents_closure_allows_duplicate_names_in_different_scopes() {
        let modules = [
            scoped_module("base", "shared", &[]),
            scoped_module("contrib", "shared", &[]),
        ];

        let affected =
            dependents_closure(&modules, &BTreeSet::from([key("contrib", "shared")]), &[]).unwrap();

        assert_eq!(affected, BTreeSet::from([key("contrib", "shared")]));
    }

    #[test]
    fn overlay_edges_expand_dependents_across_scopes() {
        let modules = [
            scoped_module("app", "api", &[]),
            scoped_module("lib", "shared", &[]),
        ];
        let overlays = [DependencyOverlay {
            from: key("app", "api"),
            to: key("lib", "shared"),
        }];

        let affected =
            dependents_closure(&modules, &BTreeSet::from([key("lib", "shared")]), &overlays)
                .unwrap();

        assert_eq!(
            affected,
            BTreeSet::from([key("app", "api"), key("lib", "shared")])
        );
    }
}
