//! The Rust wizard questionnaire: a tech-stack-driven test-runner choice.

use rskit_cli::Choice;
use rskit_errors::AppResult;
use toven_ports::{Detection, Question, QuestionKind, Questionnaire};

use crate::detect::RustFacts;

/// The question id for the cargo test-runner choice.
pub(crate) const TEST_RUNNER: &str = "test-runner";

/// The choice id for the `cargo-nextest` runner.
pub(crate) const RUNNER_NEXTEST: &str = "nextest";

/// The choice id for the built-in `cargo test` runner.
pub(crate) const RUNNER_CARGO_TEST: &str = "cargo-test";

/// Build the Rust questionnaire from a [`Detection`], preselecting the runner
/// the probe recommends: `cargo-nextest` when a `.config/nextest.toml` was
/// detected, else the built-in `cargo test`.
///
/// # Errors
/// Propagates a malformed detection-facts decode.
pub(crate) fn questionnaire(detection: &Detection) -> AppResult<Questionnaire> {
    let facts = RustFacts::from_detection(detection)?;

    let nextest = Choice::new(RUNNER_NEXTEST, "cargo-nextest");
    let nextest = if facts.nextest {
        nextest
            .with_annotation("detected .config/nextest.toml")
            .recommended()
    } else {
        nextest
    };
    let cargo_test = Choice::new(RUNNER_CARGO_TEST, "cargo test");
    let cargo_test = if facts.nextest {
        cargo_test
    } else {
        cargo_test.recommended()
    };

    let question = Question::new(
        TEST_RUNNER,
        "Which test runner should the `test` task use?",
        QuestionKind::Select(vec![nextest, cargo_test]),
    );
    Ok(Questionnaire::new(
        detection.ecosystem.clone(),
        vec![question],
    ))
}

#[cfg(test)]
mod tests {
    use toml::Table;
    use toven_ports::{Detection, QuestionKind};

    use super::{RUNNER_CARGO_TEST, RUNNER_NEXTEST, TEST_RUNNER, questionnaire};
    use crate::detect::RustFacts;

    fn detection(nextest: bool) -> Detection {
        let facts = RustFacts {
            manifest: "Cargo.toml".to_string(),
            nextest,
        };
        Detection::new(
            toven_model::EcosystemId::new("rust").unwrap(),
            Table::try_from(&facts).unwrap(),
        )
    }

    fn recommended_id(detection: &Detection) -> String {
        let questionnaire = questionnaire(detection).expect("questionnaire");
        let question = &questionnaire.questions[0];
        assert_eq!(question.id.as_str(), TEST_RUNNER);
        let QuestionKind::Select(choices) = &question.kind else {
            panic!("expected a select question");
        };
        choices
            .iter()
            .find(|choice| choice.is_recommended())
            .expect("a recommended choice")
            .id()
            .as_str()
            .to_string()
    }

    #[test]
    fn nextest_present_recommends_nextest() {
        assert_eq!(recommended_id(&detection(true)), RUNNER_NEXTEST);
    }

    #[test]
    fn nextest_absent_recommends_cargo_test() {
        assert_eq!(recommended_id(&detection(false)), RUNNER_CARGO_TEST);
    }
}
