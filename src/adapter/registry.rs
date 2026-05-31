//! Discovery adapter registry.

use std::{collections::BTreeMap, sync::Arc};

use crate::{
    adapter::{command::CommandAdapter, rust::RustAdapter},
    core::{AdapterId, AppError, AppResult, DiscoveryAdapter, Profile},
};

/// Adapter lookup by the current profile-backed scope model.
pub struct AdapterRegistry {
    builtins: BTreeMap<AdapterId, Arc<dyn DiscoveryAdapter>>,
}

impl Default for AdapterRegistry {
    fn default() -> Self {
        Self::new().with_builtin(RustAdapter::new())
    }
}

impl AdapterRegistry {
    /// Create an empty adapter registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            builtins: BTreeMap::new(),
        }
    }

    /// Register a built-in adapter.
    #[must_use]
    pub fn with_builtin(mut self, adapter: impl DiscoveryAdapter + 'static) -> Self {
        self.builtins
            .insert(adapter.adapter_id().clone(), Arc::new(adapter));
        self
    }

    /// Resolve the adapter for a profile.
    ///
    /// A configured command adapter takes precedence over a built-in adapter.
    pub fn adapter_for_profile(&self, profile: &Profile) -> AppResult<Arc<dyn DiscoveryAdapter>> {
        if let Some(command) = &profile.discovery_command {
            return Ok(Arc::new(CommandAdapter::with_field(
                profile.language.clone(),
                command.clone(),
                format!("profiles.{}.discovery_command", profile.name),
            )?));
        }

        let adapter_id = AdapterId::new(profile.language.clone())?;
        self.builtins.get(&adapter_id).cloned().ok_or_else(|| {
            AppError::invalid_input(
                format!("profiles.{}.language", profile.name),
                format!("unsupported adapter '{}'", profile.language),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::core::{ExecutionMode, Profile};

    use super::AdapterRegistry;

    fn profile(language: &str) -> Profile {
        Profile {
            name: language.to_string(),
            language: language.to_string(),
            discovery_command: None,
            execution: ExecutionMode::SpawnEach,
            module_arg_template: Vec::new(),
            resource_group: "{workspace.root}".to_string(),
            tasks: Vec::new(),
        }
    }

    #[test]
    fn resolves_builtin_rust_adapter() {
        let adapter = AdapterRegistry::default()
            .adapter_for_profile(&profile("rust"))
            .expect("rust adapter resolves");

        assert_eq!(adapter.adapter_id().as_str(), "rust");
    }

    #[test]
    fn reports_unsupported_adapter_with_profile_field() {
        let result = AdapterRegistry::default().adapter_for_profile(&profile("python"));
        let Err(error) = result else {
            panic!("unsupported adapter should fail");
        };

        assert!(error.message.contains("profiles.python.language"));
        assert!(error.message.contains("unsupported adapter"));
    }
}
