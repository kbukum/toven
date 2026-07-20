//! [`Question`] — one declarative wizard question mapped onto a `Prompter`
//! method.

use rskit_cli::Choice;
use serde::{Deserialize, Serialize};
use std::fmt;

/// A stable identifier for a [`Question`] within a
/// [`Questionnaire`](super::Questionnaire).
///
/// Opaque data (like [`ChoiceId`](rskit_cli::ChoiceId)): the adapter mints it
/// in [`questionnaire`](crate::provider::Provider::questionnaire) and reads the
/// matching [`Answer`](super::Answer) back in
/// [`render`](crate::provider::Provider::render) without stringly-typed
/// lookups.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct QuestionId(String);

impl QuestionId {
    /// Create a question identifier from any string-like value.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Borrow the identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for QuestionId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl From<String> for QuestionId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl fmt::Display for QuestionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A serializable validation rule for a [`QuestionKind::Text`] answer.
///
/// The rskit [`Validator`](rskit_cli::Validator) trait is behavioural and
/// cannot cross the wizard transport, so a `Text` question carries this data
/// instead; the CLI maps it to an rskit validator at prompt time (`NonEmpty` →
/// [`non_empty`](rskit_cli::non_empty)) so invalid input is re-asked with a
/// reason, and a rejected non-interactive default is a typed error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum TextRule {
    /// Reject empty or whitespace-only input.
    NonEmpty,
}

/// The kind of answer a [`Question`] expects, mapped one-to-one onto a
/// [`Prompter`](rskit_cli::Prompter) method by the CLI.
///
/// For [`Select`](QuestionKind::Select)/
/// [`MultiSelect`](QuestionKind::MultiSelect) the recommended default lives
/// *inside* the [`Choice`] values
/// ([`Choice::recommended`](rskit_cli::Choice::recommended)), matching how the
/// prompter resolves non-interactive defaults, so no separate `default` field
/// is needed for selection kinds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum QuestionKind {
    /// Exactly one choice (`Prompter::select`).
    Select(Vec<Choice>),
    /// Zero or more choices (`Prompter::multi_select`).
    MultiSelect(Vec<Choice>),
    /// A yes/no answer with an explicit default (`Prompter::confirm`).
    Confirm {
        /// The default answer used when the prompt is left blank or
        /// non-interactive.
        default: bool,
    },
    /// Freeform text with an optional default and optional validation rule
    /// (`Prompter::text` / `Prompter::text_with`).
    Text {
        /// The default answer, resolved directly in non-interactive mode.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<String>,
        /// An optional validation rule the CLI enforces at prompt time.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rule: Option<TextRule>,
    },
}

/// One declarative wizard question: an id, the prompt text, and its answer
/// kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    /// The stable id the [`Answer`](super::Answer) is keyed by.
    pub id: QuestionId,
    /// The human-readable prompt shown to the user.
    pub prompt: String,
    /// What kind of answer this question expects.
    pub kind: QuestionKind,
    /// An optional gate: this question is only asked when the referenced
    /// [`Confirm`](QuestionKind::Confirm) question was answered `true`.
    ///
    /// The CLI evaluates the gate against the answers collected so far, so a
    /// follow-up (e.g. a publish registry) is skipped entirely when the user
    /// declined the opt-in that precedes it; a skipped question records no
    /// answer, so [`render`](crate::provider::Provider::render) sees its
    /// default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask_if: Option<QuestionId>,
}

impl Question {
    /// Construct a question from an id, prompt, and answer kind.
    #[must_use]
    pub fn new(id: impl Into<QuestionId>, prompt: impl Into<String>, kind: QuestionKind) -> Self {
        Self {
            id: id.into(),
            prompt: prompt.into(),
            kind,
            ask_if: None,
        }
    }

    /// Gate this question on `gate`: it is only asked when the referenced
    /// [`Confirm`](QuestionKind::Confirm) question was answered `true`.
    #[must_use]
    pub fn asked_when(mut self, gate: impl Into<QuestionId>) -> Self {
        self.ask_if = Some(gate.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::{Choice, Question, QuestionKind, TextRule};

    #[test]
    fn select_question_round_trips_through_json() {
        let question = Question::new(
            "test-runner",
            "Which test runner?",
            QuestionKind::Select(vec![
                Choice::new("nextest", "cargo-nextest")
                    .with_annotation("detected")
                    .recommended(),
                Choice::new("cargo-test", "cargo test"),
            ]),
        );
        let json = serde_json::to_string(&question).expect("serialize");
        let back: Question = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(question, back);
    }

    #[test]
    fn text_question_carries_rule() {
        let question = Question::new(
            "registry",
            "Publish registry?",
            QuestionKind::Text {
                default: None,
                rule: Some(TextRule::NonEmpty),
            },
        );
        let json = serde_json::to_string(&question).expect("serialize");
        let back: Question = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(question, back);
    }

    #[test]
    fn gate_round_trips_and_is_omitted_when_absent() {
        let ungated = Question::new(
            "opt-in",
            "Enable?",
            QuestionKind::Confirm { default: false },
        );
        assert!(ungated.ask_if.is_none());
        let json = serde_json::to_string(&ungated).expect("serialize");
        assert!(
            !json.contains("ask_if"),
            "an absent gate is skipped in the wire form"
        );

        let gated = ungated.asked_when("opt-in");
        assert_eq!(
            gated.ask_if.as_ref().map(super::QuestionId::as_str),
            Some("opt-in")
        );
        let back: Question =
            serde_json::from_str(&serde_json::to_string(&gated).expect("serialize"))
                .expect("deserialize");
        assert_eq!(gated, back);
    }
}
