//! `toven init` — the config-less onboarding wizard flow.
//!
//! Point it at a repo, get a working `toven.toml`. Init runs **before** config
//! exists, so it cannot configure adapters; instead it probes a bootstrap set —
//! every in-proc [`Provider`](toven_ports::Provider) plus any `toven-<eco>`
//! driver found on `PATH` — running each provider's three-step wizard
//! ([`detect`](toven_ports::Provider::detect) →
//! [`questionnaire`](toven_ports::Provider::questionnaire) →
//! [`render`](toven_ports::Provider::render)). Questions are answered through an
//! injected [`AnswerProvider`](toven_ports::AnswerProvider) — the CLI prompts
//! interactively, tests supply canned answers — so the flow itself never prompts
//! and stays pure data orchestration. Each provider emits a complete
//! `[ecosystems.<id>]` fragment; the engine assembles `[project]`, merges the
//! fragments into one polyglot document, and renders it format-preserving.
//!
//! The flow is **minimal and additive**: a first run writes the detected
//! sections (smart defaults do the rest) with a few commented override hints; a
//! re-run adds only sections that are not already present, **warns** (never
//! touches) existing ones, **never modifies** `[project]`/`[toven]`, and
//! preserves formatting + comments. `--force <id>` regenerates one section.
//!
//! ## Modules
//! - `flow` — assemble → probe → merge → render → (optional) write.
//! - `probe` — the bootstrap probe set (in-proc providers + PATH drivers).
//! - `merge` — additive/idempotent fragment merge (format-preserving, `toml_edit`).
//! - `render` — minimal first-run emit + commented override hints.

mod flow;
mod merge;
mod probe;
mod render;

pub use flow::{InitOutcome, init, init_with};
pub use probe::ProcessDriverWizard;
