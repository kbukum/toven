//! Shared [`AnswerProvider`] double: [`ScriptedAnswers`].
//!
//! Substitutes the interactive prompt seam so init/wizard tests resolve a
//! [`Questionnaire`] deterministically without a terminal. By default it
//! answers every questionnaire with an empty [`Answers`] set (matching an
//! adapter that asks nothing); per-ecosystem scripted answers override that,
//! and the double records every questionnaire it was asked for post-hoc
//! assertions.

use std::cell::RefCell;
use std::collections::BTreeMap;

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_model::EcosystemId;
use toven_ports::{AnswerProvider, Answers, Questionnaire};

/// An [`AnswerProvider`] that replays scripted [`Answers`] per ecosystem.
///
/// Ecosystems without a scripted entry resolve to an empty answer set unless
/// the double was put in strict mode with [`strict`](Self::strict), in which
/// case an unscripted questionnaire is a hard error (modeling a missing
/// prompt).
#[derive(Debug, Default)]
pub struct ScriptedAnswers {
    scripted: BTreeMap<EcosystemId, Answers>,
    strict: bool,
    asked: RefCell<Vec<EcosystemId>>,
}

impl ScriptedAnswers {
    /// Construct a provider that answers every questionnaire with empty
    /// answers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Script `answers` for `ecosystem`'s questionnaire.
    #[must_use]
    pub fn with_answers(mut self, ecosystem: EcosystemId, answers: Answers) -> Self {
        self.scripted.insert(ecosystem, answers);
        self
    }

    /// Reject any questionnaire that has no scripted answer set.
    #[must_use]
    pub const fn strict(mut self) -> Self {
        self.strict = true;
        self
    }

    /// The ecosystems whose questionnaires were resolved, in call order.
    #[must_use]
    pub fn asked(&self) -> Vec<EcosystemId> {
        self.asked.borrow().clone()
    }
}

impl AnswerProvider for ScriptedAnswers {
    fn answers_for(&self, questionnaire: &Questionnaire) -> AppResult<Answers> {
        self.asked
            .borrow_mut()
            .push(questionnaire.ecosystem.clone());
        match self.scripted.get(&questionnaire.ecosystem) {
            Some(answers) => Ok(answers.clone()),
            None if self.strict => Err(AppError::new(
                ErrorCode::Internal,
                format!(
                    "no scripted answers for ecosystem '{}'",
                    questionnaire.ecosystem
                ),
            )),
            None => Ok(Answers::new()),
        }
    }
}
