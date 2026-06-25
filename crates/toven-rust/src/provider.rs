//! [`RustProvider`] — the stateless, id-registered cargo entry point.

use std::path::Path;

use rskit_errors::{AppError, AppResult};
use toven_model::EcosystemId;
use toven_ports::{ConfiguredAdapter, EcosystemFragment, Provider};

use crate::adapter::RustAdapter;
use crate::config::RustConfig;
use crate::scaffold;
use crate::tasks;

/// The Rust ecosystem provider: bakes `[ecosystems.rust]` into a
/// [`RustAdapter`] and self-detects a Cargo project for scaffolding.
#[derive(Debug, Clone)]
pub struct RustProvider {
    ecosystem: EcosystemId,
}

impl RustProvider {
    /// Construct the provider with the canonical `rust` ecosystem id.
    ///
    /// # Errors
    /// Returns an error only if the static `"rust"` id ever fails validation,
    /// which cannot happen for this constant.
    pub fn new() -> AppResult<Self> {
        Ok(Self {
            ecosystem: EcosystemId::new("rust")?,
        })
    }
}

impl Provider for RustProvider {
    fn ecosystem_id(&self) -> &EcosystemId {
        &self.ecosystem
    }

    fn configure(&self, raw: toml::Value) -> AppResult<Box<dyn ConfiguredAdapter>> {
        let config: RustConfig = raw.try_into().map_err(|error: toml::de::Error| {
            AppError::invalid_input("ecosystems.rust", error.to_string()).with_cause(error)
        })?;
        let tasks = tasks::resolve_tasks(&config.common.tasks)?;
        Ok(Box::new(RustAdapter::new(config, tasks)))
    }

    fn scaffold(&self, project_root: &Path) -> AppResult<Option<EcosystemFragment>> {
        scaffold::scaffold(project_root)
    }
}

#[cfg(test)]
mod tests {
    use toven_ports::Provider;

    use super::RustProvider;

    #[test]
    fn provider_serves_the_rust_ecosystem() {
        let provider = RustProvider::new().unwrap();
        assert_eq!(provider.ecosystem_id().as_str(), "rust");
    }

    #[test]
    fn configure_rejects_unknown_section_field() {
        let provider = RustProvider::new().unwrap();
        let raw: toml::Value = toml::from_str("bogus = true\n").unwrap();
        assert!(provider.configure(raw).is_err());
    }

    #[test]
    fn configure_accepts_empty_section_with_defaults() {
        let provider = RustProvider::new().unwrap();
        let raw = toml::Value::Table(toml::Table::new());
        let adapter = provider.configure(raw).expect("configures");
        assert!(!adapter.default_tasks().is_empty());
    }
}
