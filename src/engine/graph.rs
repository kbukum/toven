//! Module dependency graph validation.

use std::collections::{BTreeMap, BTreeSet};

use crate::core::{
    AppError, AppResult, Module, ModuleId, ScopedModuleKey, scoped_module_display,
    scoped_module_key,
};

pub(super) fn dependents_closure(
    modules: &[Module],
    seeds: &BTreeSet<ScopedModuleKey>,
) -> AppResult<BTreeSet<ScopedModuleKey>> {
    validate_modules(modules)?;

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

    let (_, dependents) = dependency_counts(modules);
    let mut affected = seeds.clone();
    let mut pending = seeds.iter().cloned().collect::<Vec<_>>();

    while let Some(current) = pending.pop() {
        let Some(next_modules) = dependents.get(&current) else {
            continue;
        };
        for next in next_modules {
            if affected.insert(next.clone()) {
                pending.push(next.clone());
            }
        }
    }

    Ok(affected)
}

pub(super) fn validate_modules(modules: &[Module]) -> AppResult<()> {
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
    for module in modules {
        let mut dependencies = BTreeSet::new();
        for dependency in &module.dependencies {
            if !dependencies.insert(dependency) {
                return Err(AppError::invalid_input(
                    "modules",
                    format!(
                        "module '{}' has duplicate dependency '{}'",
                        module.name, dependency
                    ),
                ));
            }
            if !selected_by_name.contains_key(dependency) {
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

    Ok(())
}

pub(super) fn dependency_counts(
    modules: &[Module],
) -> (
    BTreeMap<ScopedModuleKey, usize>,
    BTreeMap<ScopedModuleKey, Vec<ScopedModuleKey>>,
) {
    let modules_by_key = modules
        .iter()
        .map(|module| (scoped_module_key(module), module))
        .collect::<BTreeMap<_, _>>();
    let selected_by_name = selected_modules_by_name(modules_by_key.keys());
    let mut remaining = BTreeMap::new();
    let mut dependents: BTreeMap<ScopedModuleKey, Vec<ScopedModuleKey>> = BTreeMap::new();

    for (key, module) in modules_by_key {
        let dependencies = module
            .dependencies
            .iter()
            .filter_map(|dependency| dependency_key_for(module, dependency, &selected_by_name))
            .collect::<BTreeSet<_>>();
        remaining.insert(key.clone(), dependencies.len());
        for dependency in dependencies {
            dependents.entry(dependency).or_default().push(key.clone());
        }
    }

    for values in dependents.values_mut() {
        values.sort();
        values.dedup();
    }

    (remaining, dependents)
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
) -> Option<ScopedModuleKey> {
    dependency_key_for_scope(module.scope_id.as_str(), dependency, selected_by_name)
}

pub(crate) fn dependency_key_for_scope(
    scope_id: &str,
    dependency: &ModuleId,
    selected_by_name: &BTreeMap<ModuleId, Vec<ScopedModuleKey>>,
) -> Option<ScopedModuleKey> {
    let candidates = selected_by_name.get(dependency)?;
    candidates
        .iter()
        .find(|(candidate_scope_id, _)| candidate_scope_id == scope_id)
        .cloned()
        .or_else(|| (candidates.len() == 1).then(|| candidates[0].clone()))
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, path::PathBuf};

    use crate::{
        core::{AdapterId, Module, ModuleId, ScopeId, ScopedModuleKey},
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

        let affected = dependents_closure(&modules, &BTreeSet::from([key("rust", "c")])).unwrap();
        assert_eq!(
            affected,
            BTreeSet::from([key("rust", "a"), key("rust", "b"), key("rust", "c")])
        );

        let affected = dependents_closure(&modules, &BTreeSet::from([key("rust", "a")])).unwrap();
        assert_eq!(affected, BTreeSet::from([key("rust", "a")]));
    }

    #[test]
    fn dependents_closure_allows_duplicate_names_in_different_scopes() {
        let modules = [
            scoped_module("base", "shared", &[]),
            scoped_module("contrib", "shared", &[]),
        ];

        let affected =
            dependents_closure(&modules, &BTreeSet::from([key("contrib", "shared")])).unwrap();

        assert_eq!(affected, BTreeSet::from([key("contrib", "shared")]));
    }
}
