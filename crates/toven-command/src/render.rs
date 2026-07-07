//! The command wizard `render` step.
//!
//! The command adapter is explicit-config only and has no built-in task templates.
//! If rendered from a caller-supplied detection, it emits an empty opt-in section
//! that round-trips through `configure`; users then author modules/tasks manually.

use toml::Table;
use toven_ports::{Answers, Detection, EcosystemFragment};

/// Render an empty `[ecosystems.command]` fragment.
pub(crate) fn render(detection: &Detection, _answers: &Answers) -> EcosystemFragment {
    EcosystemFragment::new(detection.ecosystem.clone(), Table::new())
}

#[cfg(test)]
mod tests {
    use toven_ports::{Answers, Detection};

    use super::render;
    use crate::config::CommandConfig;

    #[test]
    fn empty_fragment_round_trips_through_config() {
        let detection = Detection::bare(toven_model::EcosystemId::new("command").unwrap());
        let fragment = render(&detection, &Answers::new());
        let config: CommandConfig = fragment
            .table
            .try_into()
            .expect("fragment parses back through CommandConfig");
        assert!(config.modules.is_empty());
        assert!(config.common.tasks.is_empty());
    }
}
