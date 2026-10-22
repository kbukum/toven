//! The JavaScript wizard questionnaire: which scripts become gates.
//!
//! The package manager is detected, never asked. The questions cover the only
//! real choices: which of the packages' scripts become Toven tasks, and
//! whether tests and type checks build workspace dependencies first.

use rskit_cli::Choice;
use rskit_errors::AppResult;
use toven_ports::{Detection, Question, QuestionKind, Questionnaire};

use crate::detect::{BUILD_SCRIPT, JsFacts};
use crate::render::{STANDARD_SCRIPTS, task_name};

/// The question id for the scripts that become tasks.
pub(crate) const GATES: &str = "gates";

/// The question id for building workspace dependencies first.
pub(crate) const DEPENDENCY_BUILDS: &str = "dependency-builds";

/// Build the JavaScript questionnaire from a [`Detection`].
///
/// - **gates**: the scripts found, each offered as a task. Standard names
///   (`build`, `typecheck`, `lint`, `format`, `test`, …) are recommended.
///   Asked only when a package declares a script.
/// - **dependency builds**: whether `build`, `test`, and `typecheck` run
///   `build` on workspace dependencies first (default yes). Asked only when a
///   package declares a `build` script.
///
/// # Errors
/// Propagates a malformed detection-facts decode.
pub(crate) fn questionnaire(detection: &Detection) -> AppResult<Questionnaire> {
    let facts = JsFacts::from_detection(detection)?;
    let mut questions = Vec::new();
    let choices: Vec<Choice> = facts
        .scripts
        .iter()
        .filter(|script| task_name(script).is_some())
        .map(|script| {
            let choice = Choice::new(script.as_str(), script.as_str());
            if STANDARD_SCRIPTS.contains(&script.as_str()) {
                choice.recommended()
            } else {
                choice
            }
        })
        .collect();
    if !choices.is_empty() {
        questions.push(Question::new(
            GATES,
            "Which package scripts should become Toven tasks?",
            QuestionKind::MultiSelect(choices),
        ));
    }
    if facts.scripts.contains(BUILD_SCRIPT) {
        questions.push(Question::new(
            DEPENDENCY_BUILDS,
            "Build workspace dependencies before `build`, `test`, and `typecheck`?",
            QuestionKind::Confirm { default: true },
        ));
    }
    Ok(Questionnaire::new(detection.ecosystem.clone(), questions))
}

#[cfg(test)]
mod tests {
    use toml::Table;
    use toven_model::EcosystemId;
    use toven_ports::{Detection, QuestionKind};

    use super::{DEPENDENCY_BUILDS, GATES, questionnaire};

    fn detection(scripts: &[&str]) -> Detection {
        let mut facts = Table::new();
        facts.insert("manager".into(), "npm".into());
        facts.insert(
            "scripts".into(),
            toml::Value::Array(scripts.iter().map(|s| (*s).into()).collect()),
        );
        facts.insert("dist".into(), false.into());
        Detection::new(EcosystemId::new("js").unwrap(), facts)
    }

    #[test]
    fn offers_found_scripts_and_recommends_standard_names() {
        let questionnaire =
            questionnaire(&detection(&["build", "deploy", "test", "weird name"])).unwrap();
        let ids: Vec<_> = questionnaire
            .questions
            .iter()
            .map(|q| q.id.as_str())
            .collect();
        assert_eq!(ids, [GATES, DEPENDENCY_BUILDS]);
        let QuestionKind::MultiSelect(choices) = &questionnaire.questions[0].kind else {
            panic!("gates is a multi-select")
        };
        let offered: Vec<_> = choices
            .iter()
            .map(|c| (c.id().as_str(), c.is_recommended()))
            .collect();
        assert_eq!(
            offered,
            [("build", true), ("deploy", false), ("test", true)]
        );
    }

    #[test]
    fn asks_nothing_without_scripts_and_no_build_question_without_build() {
        assert!(questionnaire(&detection(&[])).unwrap().questions.is_empty());
        let questionnaire = questionnaire(&detection(&["lint"])).unwrap();
        assert_eq!(questionnaire.questions.len(), 1);
        assert_eq!(questionnaire.questions[0].id.as_str(), GATES);
    }
}
