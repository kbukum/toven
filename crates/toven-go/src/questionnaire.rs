//! The Go wizard questionnaire: tech-stack-driven tool selections.
//!
//! Go's toolchain leaves several efficiency choices to the repo (which linter,
//! formatter, and test runner to drive, and whether to harden the test task).
//! Each is asked once here; the renderer authors the matching task argv from
//! the [`Answers`](toven_ports::Answers). Every default is resolved without
//! forcing an external dependency: the recommended answer is always a tool the
//! Go toolchain already ships, unless the repo already opted into an external
//! one (a detected `.golangci.yml` preselects `golangci-lint`).

use rskit_cli::Choice;
use rskit_errors::AppResult;
use toven_ports::{Detection, Question, QuestionKind, Questionnaire, release_questions};

use crate::detect::GoFacts;

/// The question id for the `lint` task backend choice.
pub(crate) const LINT_BACKEND: &str = "lint-backend";

/// The `golangci-lint` lint-backend choice id.
pub(crate) const LINT_GOLANGCI: &str = "golangci-lint";
/// The `staticcheck` lint-backend choice id.
pub(crate) const LINT_STATICCHECK: &str = "staticcheck";
/// The built-in `go vet` lint-backend choice id (drops the separate lint task).
pub(crate) const LINT_VET: &str = "vet";

/// The question id for the formatter choice.
pub(crate) const FORMATTER: &str = "formatter";

/// The `gofmt` formatter choice id.
pub(crate) const FORMAT_GOFMT: &str = "gofmt";
/// The `gofumpt` formatter choice id.
pub(crate) const FORMAT_GOFUMPT: &str = "gofumpt";
/// The `goimports` formatter choice id.
pub(crate) const FORMAT_GOIMPORTS: &str = "goimports";

/// The question id for the `test` task runner choice.
pub(crate) const TEST_RUNNER: &str = "test-runner";

/// The built-in `go test` runner choice id.
pub(crate) const RUNNER_GO_TEST: &str = "go-test";
/// The `gotestsum` runner choice id.
pub(crate) const RUNNER_GOTESTSUM: &str = "gotestsum";

/// The question id for the test-hardening confirm.
pub(crate) const TEST_HARDENING: &str = "test-hardening";

/// Build the Go questionnaire from a [`Detection`].
///
/// Asks four questions, each defaulted to a toolchain-native,
/// no-extra-dependency answer so accepting the defaults yields a working
/// catalog:
/// - **lint backend** — `golangci-lint` (preselected when a `.golangci.*`
///   config is detected), `staticcheck`, or the built-in `go vet` (recommended
///   when no config is present; selecting it drops the redundant `lint` task
///   since `check` already runs `go vet`).
/// - **formatter** — `gofmt` (recommended, always available), `gofumpt`, or
///   `goimports`.
/// - **test runner** — the built-in `go test` (recommended) or `gotestsum`.
/// - **test hardening** — whether to add `-race -shuffle=on` to the `test` task
///   (default off to keep the baseline fast; opt in for the stricter gate).
///
/// # Errors
/// Propagates a malformed detection-facts decode.
pub(crate) fn questionnaire(detection: &Detection) -> AppResult<Questionnaire> {
    let facts = GoFacts::from_detection(detection)?;

    let mut questions = vec![
        lint_backend_question(&facts),
        formatter_question(),
        test_runner_question(),
        test_hardening_question(),
    ];
    questions.extend(release_questions(None));

    Ok(Questionnaire::new(detection.ecosystem.clone(), questions))
}

/// The lint-backend select: `golangci-lint` is recommended when its config is
/// detected, otherwise the built-in `go vet` (no external dependency).
fn lint_backend_question(facts: &GoFacts) -> Question {
    let golangci = Choice::new(LINT_GOLANGCI, "golangci-lint");
    let golangci = if facts.golangci {
        golangci
            .with_annotation("detected .golangci config")
            .recommended()
    } else {
        golangci
    };
    let staticcheck = Choice::new(LINT_STATICCHECK, "staticcheck");
    let vet = Choice::new(LINT_VET, "go vet (no separate lint task)");
    let vet = if facts.golangci {
        vet
    } else {
        vet.recommended()
    };

    Question::new(
        LINT_BACKEND,
        "Which linter should the `lint` task use?",
        QuestionKind::Select(vec![golangci, staticcheck, vet]),
    )
}

/// The formatter select: `gofmt` (always available) is recommended.
fn formatter_question() -> Question {
    let gofmt = Choice::new(FORMAT_GOFMT, "gofmt").recommended();
    let gofumpt = Choice::new(FORMAT_GOFUMPT, "gofumpt");
    let goimports = Choice::new(FORMAT_GOIMPORTS, "goimports");

    Question::new(
        FORMATTER,
        "Which formatter should the `format`/`format-check` tasks use?",
        QuestionKind::Select(vec![gofmt, gofumpt, goimports]),
    )
}

/// The test-runner select: the built-in `go test` is recommended.
fn test_runner_question() -> Question {
    let go_test = Choice::new(RUNNER_GO_TEST, "go test").recommended();
    let gotestsum = Choice::new(RUNNER_GOTESTSUM, "gotestsum");

    Question::new(
        TEST_RUNNER,
        "Which test runner should the `test` task use?",
        QuestionKind::Select(vec![go_test, gotestsum]),
    )
}

/// The test-hardening confirm: add `-race -shuffle=on` to the `test` task.
fn test_hardening_question() -> Question {
    Question::new(
        TEST_HARDENING,
        "Harden the `test` task with the race detector and shuffled ordering (-race -shuffle=on)?",
        QuestionKind::Confirm { default: false },
    )
}

#[cfg(test)]
mod tests {
    use toml::Table;
    use toven_ports::{Detection, QuestionKind};

    use super::{
        LINT_BACKEND, LINT_GOLANGCI, LINT_VET, TEST_HARDENING, TEST_RUNNER, questionnaire,
    };
    use crate::detect::GoFacts;

    fn detection(golangci: bool) -> Detection {
        let facts = GoFacts {
            manifest: "go.mod".to_string(),
            golangci,
        };
        Detection::new(
            toven_model::EcosystemId::new("go").unwrap(),
            Table::try_from(&facts).unwrap(),
        )
    }

    fn recommended_id(detection: &Detection, question_id: &str) -> String {
        let questionnaire = questionnaire(detection).expect("questionnaire");
        let question = questionnaire
            .questions
            .iter()
            .find(|q| q.id.as_str() == question_id)
            .expect("question present");
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
    fn asks_the_tool_questions_then_the_release_section() {
        let questionnaire = questionnaire(&detection(false)).expect("questionnaire");
        let ids: Vec<&str> = questionnaire
            .questions
            .iter()
            .map(|q| q.id.as_str())
            .collect();
        assert_eq!(
            ids,
            [
                "lint-backend",
                "formatter",
                "test-runner",
                "test-hardening",
                "release-enabled",
                "release-prerelease",
                "release-host",
            ],
            "go is tag-only, so the release section omits the registry question",
        );
    }

    #[test]
    fn golangci_config_recommends_golangci_lint() {
        assert_eq!(
            recommended_id(&detection(true), LINT_BACKEND),
            LINT_GOLANGCI
        );
    }

    #[test]
    fn no_golangci_config_recommends_go_vet() {
        assert_eq!(recommended_id(&detection(false), LINT_BACKEND), LINT_VET);
    }

    #[test]
    fn test_runner_recommends_go_test() {
        assert_eq!(recommended_id(&detection(false), TEST_RUNNER), "go-test");
    }

    #[test]
    fn test_hardening_defaults_off() {
        let questionnaire = questionnaire(&detection(false)).expect("questionnaire");
        let question = questionnaire
            .questions
            .iter()
            .find(|q| q.id.as_str() == TEST_HARDENING)
            .expect("hardening question");
        assert!(matches!(
            question.kind,
            QuestionKind::Confirm { default: false }
        ));
    }
}
