//! Module dependency graph validation.

use std::collections::{BTreeMap, BTreeSet};

use crate::core::{AppError, AppResult, Module, ModuleId};

pub(super) fn dependents_closure(
    modules: &[Module],
    seeds: &BTreeSet<ModuleId>,
) -> AppResult<BTreeSet<ModuleId>> {
    validate_modules(modules)?;

    let module_names = modules
        .iter()
        .map(|module| module.name.clone())
        .collect::<BTreeSet<_>>();
    let unknown = seeds
        .iter()
        .filter(|seed| !module_names.contains(*seed))
        .map(ToString::to_string)
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
    let mut names = BTreeSet::new();
    for module in modules {
        if !names.insert(module.name.clone()) {
            return Err(AppError::invalid_input(
                "modules",
                format!("duplicate module '{}'", module.name),
            ));
        }
    }

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
            if !names.contains(dependency) {
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
) -> (BTreeMap<ModuleId, usize>, BTreeMap<ModuleId, Vec<ModuleId>>) {
    let mut remaining = BTreeMap::new();
    let mut dependents: BTreeMap<ModuleId, Vec<ModuleId>> = BTreeMap::new();

    for module in modules {
        let dependencies: BTreeSet<_> = module.dependencies.iter().cloned().collect();
        remaining.insert(module.name.clone(), dependencies.len());
        for dependency in dependencies {
            dependents
                .entry(dependency)
                .or_default()
                .push(module.name.clone());
        }
    }

    for values in dependents.values_mut() {
        values.sort();
        values.dedup();
    }

    (remaining, dependents)
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, path::PathBuf};

    use crate::{
        core::{Module, ModuleId},
        engine::graph::dependents_closure,
    };

    fn module(name: &str, dependencies: &[&str]) -> Module {
        Module {
            name: ModuleId::new(name).expect("module id"),
            package: Some(name.to_string()),
            root: PathBuf::from(name),
            dependencies: dependencies
                .iter()
                .map(|dependency| ModuleId::new(*dependency).expect("module id"))
                .collect(),
            source_patterns: Vec::new(),
        }
    }

    #[test]
    fn dependents_closure_walks_reverse_edges_only() {
        let modules = [module("a", &["b"]), module("b", &["c"]), module("c", &[])];

        let affected =
            dependents_closure(&modules, &BTreeSet::from([ModuleId::new("c").unwrap()])).unwrap();
        assert_eq!(
            affected,
            BTreeSet::from([
                ModuleId::new("a").unwrap(),
                ModuleId::new("b").unwrap(),
                ModuleId::new("c").unwrap()
            ])
        );

        let affected =
            dependents_closure(&modules, &BTreeSet::from([ModuleId::new("a").unwrap()])).unwrap();
        assert_eq!(affected, BTreeSet::from([ModuleId::new("a").unwrap()]));
    }
}
