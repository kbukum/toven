//! Project-level dependency overlay config.

use std::collections::BTreeSet;

use crate::core::{AppError, AppResult, DependencyOverlay, ModuleId, ScopeId, ScopedModuleKey};

/// One explicit dependency edge between scope-qualified modules.
#[derive(Debug, Clone, Eq, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DependencyOverlayConfig {
    /// Module that depends on `to`.
    pub from: ModuleRefConfig,
    /// Module required by `from`.
    pub to: ModuleRefConfig,
}

/// Scope-qualified module reference in config.
#[derive(Debug, Clone, Eq, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleRefConfig {
    /// Scope identifier.
    pub scope: String,
    /// Module identifier inside the scope.
    pub module: String,
}

pub(super) fn normalize_dependency_overlays(
    overlays: Vec<DependencyOverlayConfig>,
) -> AppResult<Vec<DependencyOverlay>> {
    let mut seen = BTreeSet::new();
    let mut normalized = Vec::with_capacity(overlays.len());

    for (index, overlay) in overlays.into_iter().enumerate() {
        let from = normalize_module_ref(&format!("overlays[{index}].from"), overlay.from)?;
        let to = normalize_module_ref(&format!("overlays[{index}].to"), overlay.to)?;
        if from == to {
            return Err(AppError::invalid_input(
                format!("overlays[{index}]"),
                "dependency overlay cannot reference the same module on both sides",
            ));
        }
        if from.0 == to.0 {
            return Err(AppError::invalid_input(
                format!("overlays[{index}]"),
                "dependency overlays must cross scope boundaries",
            ));
        }
        if !seen.insert((from.clone(), to.clone())) {
            return Err(AppError::invalid_input(
                format!("overlays[{index}]"),
                format!(
                    "duplicate dependency overlay '{} -> {}'",
                    crate::core::scoped_module_display(&from),
                    crate::core::scoped_module_display(&to)
                ),
            ));
        }
        normalized.push(DependencyOverlay { from, to });
    }

    Ok(normalized)
}

fn normalize_module_ref(field: &str, reference: ModuleRefConfig) -> AppResult<ScopedModuleKey> {
    let scope = ScopeId::new(reference.scope).map_err(|error| {
        AppError::invalid_input(format!("{field}.scope"), error.message.clone()).with_cause(error)
    })?;
    let module = ModuleId::new(reference.module).map_err(|error| {
        AppError::invalid_input(format!("{field}.module"), error.message.clone()).with_cause(error)
    })?;
    Ok((scope.to_string(), module))
}
