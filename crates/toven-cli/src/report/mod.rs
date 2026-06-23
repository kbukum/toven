//! Observability sinks + exit mapping (event-report Decisions A–C).
//!
//! The engine emits a closed, typed [`Event`](toven_model::Event) stream; these
//! sinks render it. Two built-ins ship — [`HumanReporter`] (terminal
//! tables/progress lines) and [`JsonlReporter`] (machine-parseable Event stream)
//! — and slot in future ones (GH-annotations, `JUnit`) with no engine change.
//! [`WriterRawSink`] renders the engine's per-unit raw-output channel, and
//! [`exit_code`] derives the process exit from a run summary.

mod exit;
mod human;
mod jsonl;
mod output;

pub use exit::exit_code;
pub use human::HumanReporter;
pub use jsonl::JsonlReporter;
pub use output::WriterRawSink;
