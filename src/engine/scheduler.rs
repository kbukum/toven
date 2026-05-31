//! Readiness scheduling.

use std::{collections::BTreeMap, path::PathBuf};

use crate::core::Module;

pub(super) fn split_wave_by_manifest(wave: Vec<Module>) -> Vec<Vec<Module>> {
    let mut groups = BTreeMap::<Option<PathBuf>, Vec<Module>>::new();
    for module in wave {
        groups
            .entry(module.manifest.clone())
            .or_default()
            .push(module);
    }

    groups.into_values().collect()
}
