//! [`GoProvider`] — the stateless, id-registered `go` entry point.

use std::path::Path;

use rskit_errors::{AppError, AppResult};
use toven_model::EcosystemId;
use toven_ports::{ConfiguredAdapter, EcosystemFragment, Provider};

use crate::adapter::GoAdapter;
use crate::config::GoConfig;
use crate::scaffold;
use crate::tasks;

/// The Go ecosystem provider: bakes `[ecosystems.go]` into a [`GoAdapter`] and
/// self-detects a Go module for scaffolding.
#[derive(Debug, Clone)]
pub struct GoProvider {
    ecosystem: EcosystemId,
}

impl GoProvider {
    /// Construct the provider with the canonical `go` ecosystem id.
    ///
    /// # Errors
    /// Returns an error only if the static `"go"` id ever fails validation,
    /// which cannot happen for this constant.
    pub fn new() -> AppResult<Self> {
        Ok(Self {
            ecosystem: EcosystemId::new("go")?,
        })
    }
}

impl Provider for GoProvider {
    fn ecosystem_id(&self) -> &EcosystemId {
        &self.ecosystem
    }

    fn configure(&self, raw: toml::Value) -> AppResult<Box<dyn ConfiguredAdapter>> {
        let config: GoConfig = raw.try_into().map_err(|error: toml::de::Error| {
            AppError::invalid_input("ecosystems.go", error.to_string()).with_cause(error)
        })?;
        let tasks = tasks::resolve_tasks(&config.common.tasks)?;
        Ok(Box::new(GoAdapter::new(config, tasks)))
    }

    fn scaffold(&self, project_root: &Path) -> AppResult<Option<EcosystemFragment>> {
        scaffold::scaffold(project_root)
    }
}

#[cfg(test)]
mod tests {
    use toven_ports::Provider;

    use super::GoProvider;

    #[test]
    fn provider_serves_the_go_ecosystem() {
        let provider = GoProvider::new().unwrap();
        assert_eq!(provider.ecosystem_id().as_str(), "go");
    }

    #[test]
    fn configure_rejects_unknown_section_field() {
        let provider = GoProvider::new().unwrap();
        let raw: toml::Value = toml::from_str("bogus = true\n").unwrap();
        assert!(provider.configure(raw).is_err());
    }

    #[test]
    fn configure_accepts_empty_section_with_defaults() {
        let provider = GoProvider::new().unwrap();
        let raw = toml::Value::Table(toml::Table::new());
        let adapter = provider.configure(raw).expect("configures");
        assert!(!adapter.default_tasks().is_empty());
    }
}
