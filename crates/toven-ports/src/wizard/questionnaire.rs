//! [`Questionnaire`] — an ordered set of questions for one ecosystem.

use serde::{Deserialize, Serialize};
use toven_model::EcosystemId;

use super::question::Question;

/// The ordered questions one ecosystem adapter asks during the wizard.
///
/// Built by [`Provider::questionnaire`](crate::provider::Provider::questionnaire) from a [`Detection`](super::Detection); may be empty (an ecosystem with nothing to ask still scaffolds a sane default fragment). Each [`Question`] maps one-to-one onto a [`Prompter`](rskit_cli::Prompter) method in the CLI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Questionnaire {
    /// The ecosystem these questions configure.
    pub ecosystem: EcosystemId,
    /// The ordered questions, asked top to bottom.
    pub questions: Vec<Question>,
}

impl Questionnaire {
    /// Construct a questionnaire for `ecosystem` with the given ordered
    /// questions.
    #[must_use]
    pub const fn new(ecosystem: EcosystemId, questions: Vec<Question>) -> Self {
        Self {
            ecosystem,
            questions,
        }
    }

    /// An empty questionnaire for `ecosystem` (nothing to ask).
    #[must_use]
    pub const fn empty(ecosystem: EcosystemId) -> Self {
        Self::new(ecosystem, Vec::new())
    }

    /// Whether this questionnaire asks nothing.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.questions.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{Question, Questionnaire};
    use crate::wizard::QuestionKind;
    use rskit_cli::Choice;
    use toven_model::EcosystemId;

    #[test]
    fn round_trips_through_json() {
        let questionnaire = Questionnaire::new(
            EcosystemId::new("rust").expect("id"),
            vec![Question::new(
                "runner",
                "Runner?",
                QuestionKind::Select(vec![Choice::new("nextest", "nextest").recommended()]),
            )],
        );
        let json = serde_json::to_string(&questionnaire).expect("serialize");
        let back: Questionnaire = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(questionnaire, back);
    }

    #[test]
    fn empty_questionnaire_asks_nothing() {
        let questionnaire = Questionnaire::empty(EcosystemId::new("go").expect("id"));
        assert!(questionnaire.is_empty());
    }
}
