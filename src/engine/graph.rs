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
        for dependency in &module.dependencies {
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
        remaining.insert(module.name.clone(), module.dependencies.len());
        for dependency in &module.dependencies {
            dependents
                .entry(dependency.clone())
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
