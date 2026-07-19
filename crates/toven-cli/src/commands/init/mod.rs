//! The `init` verb: run the onboarding wizard and emit/merge a `toven.toml`.
//!
//! - `run` — dispatch: build the prompt seam, drive the engine init flow, and
//!   route the [`InitOutcome`](toven_engine::init::InitOutcome) to
//!   stdout/stderr.
//! - `prompt` — the interactive [`AnswerProvider`](toven_ports::AnswerProvider)
//!   that maps each wizard [`Question`](toven_ports::Question) onto an rskit
//!   [`Prompter`](rskit_cli::Prompter) method.

mod prompt;
mod run;

pub(crate) use run::execute;
