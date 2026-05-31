//! Project dependency overlay application.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    core::{AppError, AppResult, DependencyOverlay, ScopedModuleKey, scoped_module_display},
    engine::graph::DependencyOrigin,
};

pub(super) fn apply_dependency_overlays(
    known_modules: &BTreeSet<ScopedModuleKey>,
    overlays: &[DependencyOverlay],
    ignore_missing: bool,
    dependencies: &mut BTreeMap<ScopedModuleKey, BTreeSet<ScopedModuleKey>>,
    origins: &mut BTreeMap<(ScopedModuleKey, ScopedModuleKey), DependencyOrigin>,
) -> AppResult<()> {
    for (index, overlay) in overlays.iter().enumerate() {
        if !known_modules.contains(&overlay.from) || !known_modules.contains(&overlay.to) {
            if ignore_missing {
                continue;
            }
            let missing = if known_modules.contains(&overlay.from) {
                ("to", &overlay.to)
            } else {
                ("from", &overlay.from)
            };
            return Err(AppError::invalid_input(
                format!("overlays[{index}].{}", missing.0),
                format!(
                    "dependency overlay references unknown module '{}'",
                    scoped_module_display(missing.1)
                ),
            ));
        }
        dependencies
            .entry(overlay.from.clone())
            .or_default()
            .insert(overlay.to.clone());
        origins.insert(
            (overlay.from.clone(), overlay.to.clone()),
            DependencyOrigin::Overlay,
        );
    }
    Ok(())
}
