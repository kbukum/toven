//! The Go wizard questionnaire: no choices are needed for the canonical table.

use rskit_errors::AppResult;
use toven_ports::{Detection, Questionnaire};

use crate::detect::GoFacts;

/// Build the Go questionnaire from a [`Detection`].
///
/// Go has no stack choice to ask by default: the renderer authors the canonical
/// `go` task table directly. The facts are still decoded here so malformed
/// detections fail before rendering.
///
/// # Errors
/// Propagates a malformed detection-facts decode.
pub(crate) fn questionnaire(detection: &Detection) -> AppResult<Questionnaire> {
    GoFacts::from_detection(detection)?;
    Ok(Questionnaire::empty(detection.ecosystem.clone()))
}

#[cfg(test)]
mod tests {
    use toml::Table;
    use toven_ports::Detection;

    use super::questionnaire;
    use crate::detect::GoFacts;

    #[test]
    fn go_questionnaire_is_empty() {
        let facts = GoFacts {
            manifest: "go.mod".to_string(),
        };
        let detection = Detection::new(
            toven_model::EcosystemId::new("go").unwrap(),
            Table::try_from(&facts).unwrap(),
        );

        let questionnaire = questionnaire(&detection).expect("questionnaire");
        assert!(questionnaire.is_empty());
    }
}
