//! The interactive prompt seam: an [`AnswerProvider`] backed by an rskit
//! [`Prompter`].
//!
//! `toven init` runs `detect → questionnaire → render`; this module owns the
//! **answering** step. Each wizard [`Question`](toven_ports::Question) maps
//! one-to-one onto a [`Prompter`] method by its
//! [`QuestionKind`](toven_ports::QuestionKind), and the resolved values are
//! assembled directly into [`Answers`] using the adapter-minted
//! [`ChoiceId`](rskit_cli::ChoiceId)s — no stringly-typed remapping.
//!
//! The prompter is built once and reused for the whole wizard: interactive by
//! default (rich raw-mode navigation on a TTY, else a numbered line terminal),
//! or forced [`PromptMode::NonInteractive`] by `--non-interactive`/`--yes` so a
//! CI run resolves every question to its declared default without blocking.

use std::cell::RefCell;
use std::io::stderr;

use rskit_cli::{ColorChoice, LineTerminal, Palette, PromptMode, Prompter, Terminal, non_empty};
use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_ports::{Answer, AnswerProvider, Answers, QuestionKind, Questionnaire, TextRule};

/// The CLI's interactive [`AnswerProvider`].
///
/// Wraps a single [`Prompter`] in a [`RefCell`] because the engine calls
/// [`answers_for`](AnswerProvider::answers_for) through a shared reference
/// while each prompt method needs `&mut` access to the terminal.
pub(super) struct PromptAnswers {
    prompter: RefCell<Prompter<Box<dyn Terminal>>>,
}

impl PromptAnswers {
    /// Build the prompt seam from the resolved color policy.
    ///
    /// When `non_interactive` is set the prompter is pinned to
    /// [`PromptMode::NonInteractive`] over a line terminal (every question
    /// resolves to its default); otherwise the mode follows the both-stream TTY
    /// check via [`Prompter::from_env`], selecting rich raw-mode navigation
    /// when available.
    pub(super) fn new(color: ColorChoice, non_interactive: bool) -> Self {
        let prompter = if non_interactive {
            let palette = Palette::for_stream(color, &stderr());
            Prompter::new(
                Box::new(LineTerminal::stdio()) as Box<dyn Terminal>,
                PromptMode::NonInteractive,
                palette,
            )
        } else {
            Prompter::from_env(color)
        };
        Self {
            prompter: RefCell::new(prompter),
        }
    }
}

impl AnswerProvider for PromptAnswers {
    fn answers_for(&self, questionnaire: &Questionnaire) -> AppResult<Answers> {
        let mut prompter = self.prompter.borrow_mut();
        let mut answers = Answers::new();
        for question in &questionnaire.questions {
            let answer = ask(&mut prompter, &question.prompt, &question.kind)?;
            answers.insert(question.id.clone(), answer);
        }
        Ok(answers)
    }
}

/// Ask one question through `prompter`, mapping its kind onto a prompt method.
fn ask(
    prompter: &mut Prompter<Box<dyn Terminal>>,
    prompt: &str,
    kind: &QuestionKind,
) -> AppResult<Answer> {
    let answer = match kind {
        QuestionKind::Select(choices) => Answer::Choice(prompter.select(prompt, choices)?),
        QuestionKind::MultiSelect(choices) => {
            Answer::MultiChoice(prompter.multi_select(prompt, choices)?)
        }
        QuestionKind::Confirm { default } => Answer::Bool(prompter.confirm(prompt, *default)?),
        QuestionKind::Text { default, rule } => {
            let value = match rule {
                None => prompter.text(prompt, default.as_deref())?,
                Some(TextRule::NonEmpty) => prompter.text_with(
                    prompt,
                    default.as_deref(),
                    &non_empty("a value is required"),
                )?,
                // `TextRule` is `#[non_exhaustive]`: an unmapped rule is a hard error rather than a
                // silently unenforced validation.
                Some(other) => {
                    return Err(AppError::new(
                        ErrorCode::Internal,
                        format!("init wizard cannot enforce text rule {other:?}"),
                    ));
                }
            };
            Answer::Text(value)
        }
        // `QuestionKind` is `#[non_exhaustive]`: a kind added upstream without a prompt mapping
        // here is a hard error rather than a silent drop.
        _ => {
            return Err(AppError::new(
                ErrorCode::Internal,
                format!("init wizard cannot answer question kind {kind:?}"),
            ));
        }
    };
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::{Answer, ask};
    use rskit_cli::{
        Choice, ChoiceId, Key, Palette, PromptMode, Prompter, ScriptedTerminal, Terminal,
    };
    use toven_ports::{QuestionKind, TextRule};

    fn keyed(keys: Vec<Key>) -> Prompter<Box<dyn Terminal>> {
        let terminal: Box<dyn Terminal> = Box::new(ScriptedTerminal::key_driven().with_keys(keys));
        Prompter::new(terminal, PromptMode::Interactive, Palette::new(false))
    }

    fn lined(lines: Vec<&str>) -> Prompter<Box<dyn Terminal>> {
        let terminal: Box<dyn Terminal> =
            Box::new(ScriptedTerminal::line_driven().with_lines(lines));
        Prompter::new(terminal, PromptMode::Interactive, Palette::new(false))
    }

    fn non_interactive() -> Prompter<Box<dyn Terminal>> {
        let terminal: Box<dyn Terminal> = Box::new(ScriptedTerminal::line_driven());
        Prompter::new(terminal, PromptMode::NonInteractive, Palette::new(false))
    }

    #[test]
    fn select_resolves_the_recommended_default_non_interactively() {
        let mut prompter = non_interactive();
        let kind = QuestionKind::Select(vec![
            Choice::new("nextest", "cargo-nextest").recommended(),
            Choice::new("cargo-test", "cargo test"),
        ]);
        let answer = ask(&mut prompter, "Runner?", &kind).expect("answer");
        assert_eq!(answer, Answer::Choice(ChoiceId::new("nextest")));
    }

    #[test]
    fn confirm_resolves_its_default_non_interactively() {
        let mut prompter = non_interactive();
        let answer = ask(
            &mut prompter,
            "Publish?",
            &QuestionKind::Confirm { default: true },
        )
        .expect("answer");
        assert_eq!(answer, Answer::Bool(true));
    }

    #[test]
    fn text_resolves_its_default_non_interactively() {
        let mut prompter = non_interactive();
        let kind = QuestionKind::Text {
            default: Some("crates-io".into()),
            rule: None,
        };
        let answer = ask(&mut prompter, "Registry?", &kind).expect("answer");
        assert_eq!(answer, Answer::Text("crates-io".into()));
    }

    #[test]
    fn non_empty_text_re_asks_on_blank_input() {
        // Blank first, then a value: the validator rejects the blank and re-asks.
        let mut prompter = lined(vec!["", "crates-io"]);
        let kind = QuestionKind::Text {
            default: None,
            rule: Some(TextRule::NonEmpty),
        };
        let answer = ask(&mut prompter, "Registry?", &kind).expect("answer");
        assert_eq!(answer, Answer::Text("crates-io".into()));
    }

    #[test]
    fn select_honors_an_interactive_choice() {
        // Arrow down once, then Enter: pick the second choice.
        let mut prompter = keyed(vec![Key::Down, Key::Enter]);
        let kind = QuestionKind::Select(vec![
            Choice::new("nextest", "cargo-nextest").recommended(),
            Choice::new("cargo-test", "cargo test"),
        ]);
        let answer = ask(&mut prompter, "Runner?", &kind).expect("answer");
        assert_eq!(answer, Answer::Choice(ChoiceId::new("cargo-test")));
    }
}
