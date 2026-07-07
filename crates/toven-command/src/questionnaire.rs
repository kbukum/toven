//! The command wizard questionnaire: empty because the adapter is opt-in.

use toven_ports::{Detection, Questionnaire};

/// Build the command questionnaire from a [`Detection`].
///
/// Command detections are not produced by the provider today; if a caller supplies
/// one manually, no questions are needed because all command modules/tasks are
/// user-owned explicit config.
pub(crate) fn questionnaire(detection: &Detection) -> Questionnaire {
    Questionnaire::empty(detection.ecosystem.clone())
}

#[cfg(test)]
mod tests {
    use toven_ports::Detection;

    use super::questionnaire;

    #[test]
    fn command_questionnaire_is_empty() {
        let detection = Detection::bare(toven_model::EcosystemId::new("command").unwrap());
        let questionnaire = questionnaire(&detection);
        assert!(questionnaire.is_empty());
    }
}
