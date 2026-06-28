//! `toven-cli` — the observability output layer.
//!
//! Layer 3 of the hexagonal architecture: the CLI-adjacent sinks that consume
//! the engine's typed [`Event`](toven_model::Event) stream. The engine emits
//! vocabulary only and never formats; this crate owns the rendering.
//!
//! ## Modules
//! - [`report`] — the two built-in [`Reporter`](toven_ports::Reporter) sinks
//!   ([`HumanReporter`](report::HumanReporter) /
//!   [`JsonlReporter`](report::JsonlReporter)), the terminal-bound raw-output
//!   adapter ([`WriterRawSink`](report::WriterRawSink)) that the engine's
//!   `UnitOutputChannel` writes through, and the
//!   [`exit_code`](report::exit_code) mapping from a run summary to a process
//!   [`ExitCode`](rskit_cli::ExitCode).
//! - [`grammar`] — the reserved-word set and the argv-first bare-task tail parser.
//! - [`flags`] — the clap surface (global flags + reserved-verb tree) and the
//!   per-verb applicability gate.
//! - [`collision`] — the load-time task-name / reserved-word collision warning.
//! - `commands` — the verb implementations (execution, introspection, cache,
//!   and the step-deferred stubs).
//! - The crate's [`run`] / [`run_from`] entry points tie argv → dispatch → exit
//!   code (the dispatch internals live in private `app`/`host` modules).
#![warn(missing_docs)]
// The dispatch internals (host/app/commands) live in private modules but are
// shared across sibling modules as `pub(crate)`. The `redundant_pub_crate`
// (nursery) lint would rather they be plain `pub`, but `unreachable_pub` then
// flags them as crate-internal — the two lints conflict for this shape. Allow the
// nursery lint crate-wide (the structure guard forbids per-`mod.rs` attributes)
// and keep the honest `pub(crate)` visibility.
#![allow(clippy::redundant_pub_crate)]

mod app;
pub mod collision;
pub(crate) mod commands;
pub mod flags;
pub mod grammar;
mod host;
pub mod report;

pub use app::{report_error, run, run_from};
