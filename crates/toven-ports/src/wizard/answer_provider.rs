//! [`AnswerProvider`] — the seam that answers a wizard [`Questionnaire`].
//!
//! `toven init` runs `detect → questionnaire → render` (locally or across the
//! federated driver transport), but the **answering** step is not the engine's
//! job: the CLI prompts the user (driving [`rskit_cli::Prompter`]), and tests
//! supply canned answers. Both implement this one seam, so the engine's init
//! flow and the driver wizard exchange stay agnostic about *how* an answer is
//! obtained — they only consume the resulting [`Answers`].

use rskit_errors::AppResult;

use super::{Answers, Questionnaire};

/// Resolves a wizard [`Questionnaire`] into concrete [`Answers`].
///
/// Implemented by the CLI (interactive prompting or non-interactive default
/// resolution via [`rskit_cli::PromptMode`]) and by tests (canned answers). The
/// engine calls it exactly once per detected ecosystem, between that
/// ecosystem's `questionnaire` and `render` steps.
pub trait AnswerProvider {
    /// Answer every question in `questionnaire`, returning the resolved set.
    ///
    /// # Errors
    /// Returns a typed error when a required, defaultless question cannot be
    /// answered (for example a non-interactive run of a question with no
    /// recommended choice), or when interactive input fails.
    fn answers_for(&self, questionnaire: &Questionnaire) -> AppResult<Answers>;
}
