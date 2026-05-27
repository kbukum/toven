//! Module dependency graph validation.

use std::collections::{BTreeMap, BTreeSet};

use crate::core::{AppError, AppResult, Module, ModuleId};

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
