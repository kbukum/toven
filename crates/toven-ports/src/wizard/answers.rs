//! [`Answers`] — the user's resolved answers to one ecosystem's questionnaire.

use std::collections::BTreeMap;

use rskit_cli::ChoiceId;
use serde::{Deserialize, Serialize};

use super::question::QuestionId;

/// A single resolved answer, keyed in [`Answers`] by its [`QuestionId`].
///
/// The variant matches the question's
/// [`QuestionKind`](super::QuestionKind): the carried [`ChoiceId`]s are exactly
/// those the adapter minted in its [`Choice`](rskit_cli::Choice)s, so
/// [`render`](crate::provider::Provider::render) matches them without
/// stringly-typed lookups.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Answer {
    /// The chosen id for a [`Select`](super::QuestionKind::Select) question.
    Choice(ChoiceId),
    /// The chosen ids for a [`MultiSelect`](super::QuestionKind::MultiSelect) question.
    MultiChoice(Vec<ChoiceId>),
    /// The answer to a [`Confirm`](super::QuestionKind::Confirm) question.
    Bool(bool),
    /// The answer to a [`Text`](super::QuestionKind::Text) question.
    Text(String),
}

/// The user's answers to one ecosystem's [`Questionnaire`](super::Questionnaire),
/// keyed by [`QuestionId`].
///
/// Assembled by the CLI from prompt results (or by resolving
/// [`PromptMode::NonInteractive`](rskit_cli::PromptMode) defaults) and handed to
/// [`render`](crate::provider::Provider::render).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answers {
    map: BTreeMap<QuestionId, Answer>,
}

impl Answers {
    /// An empty answer set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record `answer` for `id`, replacing any prior answer for that question.
    pub fn insert(&mut self, id: impl Into<QuestionId>, answer: Answer) {
        self.map.insert(id.into(), answer);
    }

    /// Builder form of [`insert`](Self::insert).
    #[must_use]
    pub fn with(mut self, id: impl Into<QuestionId>, answer: Answer) -> Self {
        self.insert(id, answer);
        self
    }

    /// The answer for `id`, if any.
    #[must_use]
    pub fn get(&self, id: &QuestionId) -> Option<&Answer> {
        self.map.get(id)
    }

    /// The chosen [`ChoiceId`] for a [`Select`](super::QuestionKind::Select)
    /// answer at `id`, if present and of the matching kind.
    #[must_use]
    pub fn choice(&self, id: &QuestionId) -> Option<&ChoiceId> {
        match self.map.get(id) {
            Some(Answer::Choice(choice)) => Some(choice),
            _ => None,
        }
    }

    /// The text of a [`Text`](super::QuestionKind::Text) answer at `id`, if
    /// present and of the matching kind.
    #[must_use]
    pub fn text(&self, id: &QuestionId) -> Option<&str> {
        match self.map.get(id) {
            Some(Answer::Text(text)) => Some(text.as_str()),
            _ => None,
        }
    }

    /// The value of a [`Confirm`](super::QuestionKind::Confirm) answer at `id`,
    /// if present and of the matching kind.
    #[must_use]
    pub fn bool(&self, id: &QuestionId) -> Option<bool> {
        match self.map.get(id) {
            Some(Answer::Bool(value)) => Some(*value),
            _ => None,
        }
    }

    /// Whether no answers have been recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{Answer, Answers};
    use rskit_cli::ChoiceId;

    #[test]
    fn typed_accessors_match_only_their_kind() {
        let answers = Answers::new()
            .with("runner", Answer::Choice(ChoiceId::new("nextest")))
            .with("publish", Answer::Bool(true))
            .with("registry", Answer::Text("crates-io".into()));

        assert_eq!(
            answers.choice(&"runner".into()),
            Some(&ChoiceId::new("nextest"))
        );
        assert_eq!(answers.bool(&"publish".into()), Some(true));
        assert_eq!(answers.text(&"registry".into()), Some("crates-io"));
        // Wrong-kind lookups yield None rather than a panic.
        assert!(answers.text(&"runner".into()).is_none());
        assert!(answers.choice(&"publish".into()).is_none());
    }

    #[test]
    fn round_trips_through_json() {
        let answers = Answers::new().with("runner", Answer::Choice(ChoiceId::new("nextest")));
        let json = serde_json::to_string(&answers).expect("serialize");
        let back: Answers = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(answers, back);
    }
}
