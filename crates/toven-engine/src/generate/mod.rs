//! `toven generate` — the config-less onboarding flow.
//!
//! Point it at a repo, get a working `toven.toml`. Generation runs **before**
//! config exists, so it cannot configure adapters; instead it probes a bootstrap
//! set — every in-proc [`Provider`](toven_ports::Provider) plus any `toven-<eco>`
//! driver found on `PATH` — and each self-detects whether it applies via
//! [`Provider::scaffold`](toven_ports::Provider::scaffold), emitting a minimal
//! `[ecosystems.<id>]` fragment. The engine assembles `[project]`, merges the
//! fragments into one polyglot document, and renders it format-preserving.
//!
//! The flow is **minimal and additive**: a first run writes only the discovery
//! hints (smart defaults do the rest) with a few commented override hints; a
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

pub use flow::{GeneratedDocument, generate, generate_with};
pub use probe::{DriverScaffolder, ProcessDriverScaffolder};
