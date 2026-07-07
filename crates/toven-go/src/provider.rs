//! [`GoProvider`] — the stateless, id-registered `go` entry point.

use std::path::Path;

use rskit_config::{RawValue, deserialize_subtree};
use rskit_errors::AppResult;
use toven_model::EcosystemId;
use toven_ports::{
    Answers, ConfiguredAdapter, Detection, EcosystemFragment, Provider, Questionnaire,
};

use crate::adapter::GoAdapter;
use crate::config::GoConfig;
use crate::{detect, questionnaire, render};

/// The Go ecosystem provider: bakes `[ecosystems.go]` into a [`GoAdapter`] and
/// drives the Go onboarding wizard.
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

    fn configure(&self, raw: RawValue) -> AppResult<Box<dyn ConfiguredAdapter>> {
        let config: GoConfig = deserialize_subtree("ecosystems.go", raw)?;
        for (key, entry) in &config.common.tasks {
            entry.materialize("go", key)?;
        }
        Ok(Box::new(GoAdapter::new(config)))
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
    use rskit_config::RawValue;
    use toven_ports::Provider;

    use super::GoProvider;

    fn raw_subtree(toml: &str) -> RawValue {
        rskit_codec::decode(&rskit_codec::TomlCodec, toml).expect("raw subtree")
    }

    #[test]
    fn provider_serves_the_go_ecosystem() {
        let provider = GoProvider::new().unwrap();
        assert_eq!(provider.ecosystem_id().as_str(), "go");
    }

    #[test]
    fn configure_rejects_unknown_section_field() {
        let provider = GoProvider::new().unwrap();
        let raw = raw_subtree("bogus = true");
        assert!(provider.configure(raw).is_err());
    }

    #[test]
    fn configure_accepts_empty_section_with_defaults() {
        let provider = GoProvider::new().unwrap();
        let raw = raw_subtree("");
        provider.configure(raw).expect("configures");
    }

    #[test]
    fn configure_rejects_task_entry_with_empty_argv() {
        let provider = GoProvider::new().unwrap();
        let raw = raw_subtree("[tasks.test]\nargv = []\n");
        let Err(error) = provider.configure(raw) else {
            panic!("empty argv should be rejected")
        };
        assert!(
            error.to_string().contains("ecosystems.go.tasks.test"),
            "{error}"
        );
    }
}
