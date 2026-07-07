//! [`CommandProvider`] — the stateless, id-registered escape-hatch entry point.

use std::path::Path;

use rskit_config::{RawValue, deserialize_subtree};
use rskit_errors::{AppError, AppResult};
use toven_model::EcosystemId;
use toven_ports::{
    Answers, ConfiguredAdapter, Detection, EcosystemFragment, Provider, Questionnaire,
};

use crate::adapter::CommandAdapter;
use crate::config::CommandConfig;
use crate::{detect, questionnaire, render};

/// The command ecosystem provider: bakes `[ecosystems.command]` into a
/// [`CommandAdapter`].
///
/// Unlike the tooling-backed providers it self-detects nothing — there is no
/// canonical command project on disk — so [`Provider::detect`] always returns
/// `None`.
#[derive(Debug, Clone)]
pub struct CommandProvider {
    ecosystem: EcosystemId,
}

impl CommandProvider {
    /// Construct the provider with the canonical `command` ecosystem id.
    ///
    /// # Errors
    /// Returns an error only if the static `"command"` id ever fails validation,
    /// which cannot happen for this constant.
    pub fn new() -> AppResult<Self> {
        Ok(Self {
            ecosystem: EcosystemId::new("command")?,
        })
    }
}

impl Provider for CommandProvider {
    fn ecosystem_id(&self) -> &EcosystemId {
        &self.ecosystem
    }

    fn configure(&self, raw: RawValue) -> AppResult<Box<dyn ConfiguredAdapter>> {
        let config: CommandConfig = deserialize_subtree("ecosystems.command", raw)?;
        for (key, entry) in &config.common.tasks {
            entry.materialize("command", key)?;
        }
        // A command project that declares modules but neither tasks nor an
        // explicit `[toolchain]` has no probeable toolchain, yet its modules
        // would still be probed during toolchain resolution (which runs before
        // scheduling). Reject that at the config boundary with an actionable
        // error instead of letting PLAN fail on an un-runnable degenerate probe.
        if !config.modules.is_empty()
            && config.common.tasks.is_empty()
            && config.toolchain.is_none()
        {
            return Err(AppError::invalid_input(
                "ecosystems.command",
                "declares modules but no tasks or [toolchain]: add at least one [tasks.*] or a \
                 [toolchain] block so the workspace has a probeable toolchain identity",
            ));
        }
        Ok(Box::new(CommandAdapter::new(config)))
    }

    fn detect(&self, project_root: &Path) -> AppResult<Option<Detection>> {
        Ok(detect::detect(project_root))
    }

    fn questionnaire(&self, detection: &Detection) -> AppResult<Questionnaire> {
        Ok(questionnaire::questionnaire(detection))
    }

    fn render(&self, detection: &Detection, answers: &Answers) -> AppResult<EcosystemFragment> {
        Ok(render::render(detection, answers))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rskit_config::RawValue;
    use toven_ports::Provider;

    use super::CommandProvider;

    fn raw_subtree(toml: &str) -> RawValue {
        rskit_codec::decode(&rskit_codec::TomlCodec, toml).expect("raw subtree")
    }

    #[test]
    fn provider_serves_the_command_ecosystem() {
        let provider = CommandProvider::new().unwrap();
        assert_eq!(provider.ecosystem_id().as_str(), "command");
    }

    #[test]
    fn configure_rejects_unknown_section_field() {
        let provider = CommandProvider::new().unwrap();
        let raw = raw_subtree("bogus = true");
        assert!(provider.configure(raw).is_err());
    }

    #[test]
    fn configure_yields_no_builtin_tasks() {
        let provider = CommandProvider::new().unwrap();
        let raw = raw_subtree("");
        let adapter = provider.configure(raw).expect("configures");
        assert!(adapter.common().tasks.is_empty());
    }

    #[test]
    fn never_detects() {
        let provider = CommandProvider::new().unwrap();
        assert!(provider.detect(Path::new(".")).unwrap().is_none());
    }

    #[test]
    fn configure_rejects_task_entry_with_empty_argv() {
        let provider = CommandProvider::new().unwrap();
        let raw = raw_subtree("[tasks.test]\nargv = []\n");
        let Err(error) = provider.configure(raw) else {
            panic!("empty argv should be rejected")
        };
        assert!(
            error.to_string().contains("ecosystems.command.tasks.test"),
            "{error}"
        );
    }
}
