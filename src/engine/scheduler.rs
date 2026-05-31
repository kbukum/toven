//! Readiness scheduling.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    core::{AppError, AppResult, Module, ModuleId},
    engine::graph::{dependency_counts, validate_modules},
};

pub(super) fn ready_waves(modules: &[Module]) -> AppResult<Vec<Vec<Module>>> {
    validate_modules(modules)?;

    let modules_by_id: BTreeMap<ModuleId, Module> = modules
        .iter()
        .map(|module| (module.name.clone(), module.clone()))
        .collect();
    let (mut remaining, dependents) = dependency_counts(modules);
    let mut ready: BTreeSet<ModuleId> = remaining
        .iter()
        .filter_map(|(name, count)| (*count == 0).then_some(name.clone()))
        .collect();
    let mut satisfied = BTreeSet::new();
    let mut waves = Vec::new();

    while !ready.is_empty() {
        let current = std::mem::take(&mut ready);
        let mut wave = Vec::with_capacity(current.len());

        for name in &current {
            satisfied.insert(name.clone());
            remaining.remove(name);
            if let Some(module) = modules_by_id.get(name) {
                wave.push(module.clone());
            }
        }
        waves.push(wave);

        for name in current {
            if let Some(next_modules) = dependents.get(&name) {
                for next in next_modules {
                    if satisfied.contains(next) {
                        continue;
                    }
                    let Some(count) = remaining.get_mut(next) else {
                        continue;
                    };
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        ready.insert(next.clone());
                    }
                }
            }
        }
    }

    if !remaining.is_empty() {
        let modules = remaining
            .keys()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(AppError::invalid_input(
            "modules",
            format!("module dependency cycle detected among: {modules}"),
        ));
    }

    Ok(waves)
}

#[cfg(test)]
mod tests {
    use crate::{
        core::{Module, ModuleId},
        engine::scheduler::ready_waves,
    };

    fn module(name: &str, dependencies: &[&str]) -> Module {
        Module {
            name: ModuleId::new(name).expect("module id"),
            package: Some(name.to_string()),
            root: name.into(),
            manifest: Some("Cargo.toml".into()),
            dependencies: dependencies
                .iter()
                .map(|dependency| ModuleId::new(*dependency).expect("module id"))
                .collect(),
            source_patterns: Vec::new(),
        }
    }

    #[test]
    fn releases_dependency_waves_in_order() {
        let waves = ready_waves(&[
            module("a", &["b"]),
            module("b", &["c"]),
            module("c", &["d"]),
            module("d", &[]),
        ])
        .expect("waves schedule");

        let names: Vec<Vec<_>> = waves
            .iter()
            .map(|wave| {
                wave.iter()
                    .map(|module| module.name.as_str().to_string())
                    .collect()
            })
            .collect();
        assert_eq!(names, [["d"], ["c"], ["b"], ["a"]]);
    }

    #[test]
    fn rejects_unknown_dependencies() {
        let error =
            ready_waves(&[module("a", &["missing"])]).expect_err("unknown dependency should fail");

        assert!(error.message.contains("unknown module"));
    }

    #[test]
    fn rejects_duplicate_dependencies() {
        let error = ready_waves(&[module("a", &["b", "b"]), module("b", &[])])
            .expect_err("duplicate dependency should fail");

        assert!(error.message.contains("duplicate dependency"));
    }

    #[test]
    fn rejects_cycles() {
        let error = ready_waves(&[module("a", &["b"]), module("b", &["a"])])
            .expect_err("cycle should fail");

        assert!(error.message.contains("cycle"));
    }
}
