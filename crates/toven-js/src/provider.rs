//! [`JsProvider`] — the stateless, id-registered `js` entry point.

use std::path::Path;
use std::sync::Arc;

use rskit_config::{RawValue, deserialize_subtree};
use rskit_errors::AppResult;
use toven_model::EcosystemId;
use toven_ports::{
    Answers, ConfiguredAdapter, Detection, EcosystemFragment, Provider, Questionnaire, ToolRunner,
};

use crate::adapter::JsAdapter;
use crate::config::JsConfig;
use crate::discovery::JS;
use crate::release::pack::PackCommand;

/// The JavaScript/TypeScript ecosystem provider: bakes `[ecosystems.js]` into
/// a [`JsAdapter`].
#[derive(Clone)]
pub struct JsProvider {
    ecosystem: EcosystemId,
    runner: Arc<dyn ToolRunner>,
}

impl JsProvider {
    /// Construct the provider with the canonical `js` ecosystem id. `runner`
    /// executes release commands (pack, publish, lockfile refresh); discovery
    /// never uses it.
    ///
    /// # Errors
    /// Returns an error only if the static `"js"` id ever fails validation,
    /// which cannot happen for this constant.
    pub fn new(runner: Arc<dyn ToolRunner>) -> AppResult<Self> {
        Ok(Self {
            ecosystem: EcosystemId::new(JS)?,
            runner,
        })
    }
}

impl Provider for JsProvider {
    fn ecosystem_id(&self) -> &EcosystemId {
        &self.ecosystem
    }

    fn configure(&self, raw: RawValue) -> AppResult<Box<dyn ConfiguredAdapter>> {
        let config: JsConfig = deserialize_subtree("ecosystems.js", raw)?;
        for (key, entry) in &config.common.tasks {
            entry.materialize(JS, key)?;
        }
        for (root, argv) in &config.pack {
            PackCommand::parse(&format!("ecosystems.js.pack.{root}"), argv)?;
        }
        Ok(Box::new(JsAdapter::new(config, self.runner.clone())))
    }

    fn detect(&self, project_root: &Path) -> AppResult<Option<Detection>> {
        crate::detect::detect(project_root)
    }

    fn questionnaire(&self, detection: &Detection) -> AppResult<Questionnaire> {
        crate::questionnaire::questionnaire(detection)
    }

    fn render(&self, detection: &Detection, answers: &Answers) -> AppResult<EcosystemFragment> {
        crate::render::render(detection, answers)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rskit_config::RawValue;
    use toven_ports::Provider;
    use toven_testkit::FakeToolRunner;

    use super::JsProvider;

    fn provider() -> JsProvider {
        JsProvider::new(Arc::new(FakeToolRunner::new())).expect("provider")
    }

    fn raw_subtree(toml: &str) -> RawValue {
        rskit_codec::decode(&rskit_codec::TomlCodec, toml).expect("raw subtree")
    }

    #[test]
    fn provider_serves_the_js_ecosystem() {
        assert_eq!(provider().ecosystem_id().as_str(), "js");
    }

    #[test]
    fn configure_accepts_an_empty_section() {
        provider().configure(raw_subtree("")).expect("configures");
    }

    #[test]
    fn configure_rejects_an_invalid_pack_command() {
        let provider = provider();
        provider
            .configure(raw_subtree(
                "[pack]\n\".\" = [\"bun\", \"pack.ts\", \"{module.root}\", \"{destination}\"]\n",
            ))
            .expect("valid pack command");
        let Err(error) = provider.configure(raw_subtree("[pack]\nweb = [\"bun\", \"pack.ts\"]\n"))
        else {
            panic!("a pack command without {{destination}} should be rejected")
        };
        assert!(
            error.to_string().contains("ecosystems.js.pack.web"),
            "{error}"
        );
    }

    #[test]
    fn configure_rejects_unknown_fields_and_empty_argv() {
        let provider = provider();
        assert!(provider.configure(raw_subtree("bogus = true")).is_err());
        let Err(error) = provider.configure(raw_subtree("[tasks.test]\nargv = []\n")) else {
            panic!("empty argv should be rejected")
        };
        assert!(
            error.to_string().contains("ecosystems.js.tasks.test"),
            "{error}"
        );
    }
}
