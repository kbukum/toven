//! Wizard vocabulary — the data-only, three-step onboarding contract.
//!
//! The [`Provider`](crate::provider::Provider) wizard replaces the old
//! single-shot scaffold with three declarative steps whose values are all
//! serializable, so they cross the federated driver transport unchanged:
//!
//! 1. [`detect`](crate::provider::Provider::detect) → [`Detection`] — probe the
//!    ecosystem and record adapter-owned facts.
//! 2. [`questionnaire`](crate::provider::Provider::questionnaire) →
//!    [`Questionnaire`] of [`Question`]s — declarative choices the CLI prompts.
//! 3. [`render`](crate::provider::Provider::render) from [`Answers`] →
//!    [`EcosystemFragment`](crate::provider::EcosystemFragment) — the complete
//!    `[ecosystems.<id>]` section.
//!
//! The selection/choice vocabulary is **reused from [`rskit_cli::prompt`]**
//! ([`Choice`](rskit_cli::Choice) / [`ChoiceId`](rskit_cli::ChoiceId)) rather
//! than redefined here: the same [`Choice`](rskit_cli::Choice) the adapter
//! authors is the exact value the CLI feeds to
//! [`Prompter::select`](rskit_cli::Prompter::select), and it already carries
//! `annotation` + `recommended` and (de)serializes for the transport.

mod answer_provider;
mod answers;
mod detection;
mod question;
mod questionnaire;
mod release;

pub use answer_provider::AnswerProvider;
pub use answers::{Answer, Answers};
pub use detection::Detection;
pub use question::{Question, QuestionId, QuestionKind, TextRule};
pub use questionnaire::Questionnaire;
pub use release::{
    RELEASE_ENABLED, RELEASE_HOST, RELEASE_PRERELEASE, RELEASE_REGISTRY, REGISTRY_NONE,
    release_config, release_questions,
};
