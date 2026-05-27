//! Language adapter registry.

use std::{collections::BTreeMap, sync::Arc};

use crate::{
    core::{AppError, AppResult, LangAdapter, Profile},
    lang::{command::CommandAdapter, rust::RustAdapter},
};

/// Adapter lookup by language profile.
pub struct LangRegistry {
    builtins: BTreeMap<String, Arc<dyn LangAdapter>>,
}

impl Default for LangRegistry {
    fn default() -> Self {
        Self::new().with_builtin(RustAdapter::new())
    }
}

impl LangRegistry {
    /// Create an empty language registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            builtins: BTreeMap::new(),
        }
    }

    /// Register a built-in adapter.
    #[must_use]
    pub fn with_builtin(mut self, adapter: impl LangAdapter + 'static) -> Self {
        self.builtins
            .insert(adapter.language().to_string(), Arc::new(adapter));
        self
    }

    /// Resolve the adapter for a profile.
    ///
    /// A configured command adapter takes precedence over a built-in adapter.
    pub fn adapter_for_profile(&self, profile: &Profile) -> AppResult<Arc<dyn LangAdapter>> {
        if let Some(command) = &profile.discovery_command {
            return Ok(Arc::new(CommandAdapter::with_field(
                profile.language.clone(),
                command.clone(),
                format!("profiles.{}.discovery_command", profile.name),
            )?));
        }

        self.builtins
            .get(&profile.language)
            .cloned()
            .ok_or_else(|| {
                AppError::invalid_input(
                    format!("profiles.{}.language", profile.name),
                    format!("unsupported language '{}'", profile.language),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use crate::core::{ExecutionMode, Profile};

    use super::LangRegistry;

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
        let adapter = LangRegistry::default()
            .adapter_for_profile(&profile("rust"))
            .expect("rust adapter resolves");

        assert_eq!(adapter.language(), "rust");
    }

    #[test]
    fn reports_unsupported_language_with_profile_field() {
        let result = LangRegistry::default().adapter_for_profile(&profile("python"));
        let Err(error) = result else {
            panic!("unsupported language should fail");
        };

        assert!(error.message.contains("profiles.python.language"));
        assert!(error.message.contains("unsupported language"));
    }
}
