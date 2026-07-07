//! [`RustProvider`] — the stateless, id-registered cargo entry point.

use std::path::Path;

use rskit_config::{RawValue, deserialize_subtree};
use rskit_errors::AppResult;
use toven_model::EcosystemId;
use toven_ports::{
    Answers, ConfiguredAdapter, Detection, EcosystemFragment, Provider, Questionnaire,
};

use crate::adapter::RustAdapter;
use crate::config::RustConfig;
use crate::{detect, questionnaire, render};

/// The Rust ecosystem provider: bakes `[ecosystems.rust]` into a
/// [`RustAdapter`] and drives the cargo onboarding wizard.
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
        // Fail closed on an incomplete task entry (e.g. empty argv) at configure
        // time, citing the offending `ecosystems.rust.tasks.<name>` path.
        for (key, entry) in &config.common.tasks {
            entry.materialize("rust", key)?;
        }
        Ok(Box::new(RustAdapter::new(config)))
    }

    fn detect(&self, project_root: &Path) -> AppResult<Option<Detection>> {
        detect::detect(project_root)
    }

    fn questionnaire(&self, detection: &Detection) -> AppResult<Questionnaire> {
        questionnaire::questionnaire(detection)
    }

    fn render(&self, detection: &Detection, answers: &Answers) -> AppResult<EcosystemFragment> {
        render::render(detection, answers)
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
        provider.configure(raw).expect("configures");
    }

    #[test]
    fn configure_rejects_task_entry_with_empty_argv() {
        let provider = RustProvider::new().unwrap();
        let raw = toven_testkit::raw_subtree("[tasks.test]\nargv = []\n").expect("subtree");
        let Err(error) = provider.configure(raw) else {
            panic!("empty argv should be rejected")
        };
        assert!(
            error.to_string().contains("ecosystems.rust.tasks.test"),
            "{error}"
        );
    }
}
