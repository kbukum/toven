//! [`RustProvider`] — the stateless, id-registered cargo entry point.

use std::path::Path;

use rskit_config::{RawValue, deserialize_subtree};
use rskit_errors::AppResult;
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

    fn configure(&self, raw: RawValue) -> AppResult<Box<dyn ConfiguredAdapter>> {
        let config: RustConfig = deserialize_subtree("ecosystems.rust", raw)?;
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
        let raw = toven_testkit::raw_subtree("bogus = true").expect("subtree");
        assert!(provider.configure(raw).is_err());
    }

    #[test]
    fn configure_accepts_empty_section_with_defaults() {
        let provider = RustProvider::new().unwrap();
        let raw = toven_testkit::raw_subtree("").expect("subtree");
        let adapter = provider.configure(raw).expect("configures");
        assert!(!adapter.default_tasks().is_empty());
    }
}
