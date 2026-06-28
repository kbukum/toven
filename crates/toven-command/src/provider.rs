//! [`CommandProvider`] — the stateless, id-registered escape-hatch entry point.

use std::path::Path;

use rskit_errors::{AppError, AppResult};
use toven_model::EcosystemId;
use toven_ports::{ConfiguredAdapter, EcosystemFragment, Provider};

use crate::adapter::CommandAdapter;
use crate::config::CommandConfig;
use crate::tasks;

/// The command ecosystem provider: bakes `[ecosystems.command]` into a
/// [`CommandAdapter`].
///
/// Unlike the tooling-backed providers it self-detects nothing — there is no
/// canonical command project on disk — so [`Provider::scaffold`] always returns
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

    fn configure(&self, raw: toml::Value) -> AppResult<Box<dyn ConfiguredAdapter>> {
        let config: CommandConfig = raw.try_into().map_err(|error: toml::de::Error| {
            AppError::invalid_input("ecosystems.command", error.to_string()).with_cause(error)
        })?;
        let tasks = tasks::resolve_tasks(&config.common.tasks)?;
        // A command project that declares modules but neither tasks nor an
        // explicit `[toolchain]` has no probeable toolchain, yet its modules
        // would still be probed during toolchain resolution (which runs before
        // scheduling). Reject that at the config boundary with an actionable
        // error instead of letting PLAN fail on an un-runnable degenerate probe.
        if !config.modules.is_empty() && tasks.is_empty() && config.toolchain.is_none() {
            return Err(AppError::invalid_input(
                "ecosystems.command",
                "declares modules but no tasks or [toolchain]: add at least one [tasks.*] or a \
                 [toolchain] block so the workspace has a probeable toolchain identity",
            ));
        }
        Ok(Box::new(CommandAdapter::new(config, tasks)))
    }

    fn scaffold(&self, _project_root: &Path) -> AppResult<Option<EcosystemFragment>> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use toven_ports::Provider;

    use super::CommandProvider;

    #[test]
    fn provider_serves_the_command_ecosystem() {
        let provider = CommandProvider::new().unwrap();
        assert_eq!(provider.ecosystem_id().as_str(), "command");
    }

    #[test]
    fn configure_rejects_unknown_section_field() {
        let provider = CommandProvider::new().unwrap();
        let raw: toml::Value = toml::from_str("bogus = true\n").unwrap();
        assert!(provider.configure(raw).is_err());
    }

    #[test]
    fn configure_yields_no_builtin_tasks() {
        let provider = CommandProvider::new().unwrap();
        let raw = toml::Value::Table(toml::Table::new());
        let adapter = provider.configure(raw).expect("configures");
        assert!(adapter.default_tasks().is_empty());
    }

    #[test]
    fn never_scaffolds() {
        let provider = CommandProvider::new().unwrap();
        assert!(provider.scaffold(Path::new(".")).unwrap().is_none());
    }
}
