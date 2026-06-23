//! `toven-cli` — the observability output layer (event-report.md, step 7).
//!
//! Layer 3 of the hexagonal architecture: the CLI-adjacent sinks that consume
//! the engine's typed [`Event`](toven_model::Event) stream. The engine emits
//! vocabulary only and never formats; this crate owns the rendering.
//!
//! ## Modules
//! - [`report`] — the two built-in [`Reporter`](toven_ports::Reporter) sinks
//!   ([`HumanReporter`](report::HumanReporter) /
//!   [`JsonlReporter`](report::JsonlReporter)), the terminal-bound raw-output
//!   adapter ([`WriterRawSink`](report::WriterRawSink)) the engine's
//!   `UnitOutputChannel` writes through, and the
//!   [`exit_code`](report::exit_code) mapping from a run summary to a process
//!   [`ExitCode`](rskit_cli::ExitCode).
#![warn(missing_docs)]

pub mod report;
