//! `toven-runtime` — the one generic unit-operation engine.
//!
//! Multi-module work in Toven decomposes into the same two phases: a **shared
//! GATHER** that resolves the verb's workspace-coupled prerequisites exactly
//! once, then a **per-unit STREAM** that processes each unit and emits its
//! settled outcome the instant it lands, parallelized within the dependency
//! graph and bounded by a job limit.
//!
//! This crate owns that shape once, so a verb streams per-unit outcomes without
//! hand-rolling a second executor. The streamed `release` verbs run on it today
//! — `status`, `readiness`, and the artifact verbs (`sbom`, `depgraphs`,
//! `package`, `checksums`, `sign`, `verify`, `image`, `provenance`). `run` and
//! `coverage` still use their own executors and are not yet migrated.
//!
//! The engine's parts:
//!
//! - [`UnitSpec`] + [`level_waves`] — the unit graph and its dependency-wave
//!   levelling (an edgeless graph collapses to one wide parallel wave; an edged
//!   graph runs dependency-ordered waves). The engine reads the edges; it never
//!   special-cases a verb.
//! - [`Gate`] — fail-closed reverse-dependency gating: a failed unit blocks only
//!   its transitive dependents, never its dependencies.
//! - [`UnitOperation`] — the per-verb seam: its associated `Shared` GATHER value
//!   (resolved once) and its per-unit [`Completed`] outcome carrying a typed,
//!   per-family payload, so a new per-module verb streams without inventing new
//!   engine variants.
//! - [`Progress`] + [`UnitReport`] — the generic per-unit lifecycle contract
//!   (`started` then `settled`) the consuming layer projects to its own event
//!   vocabulary and output sinks.
//! - [`execute`] — the driver: gather once, then stream wave-scheduled,
//!   bounded-parallel per-unit outcomes, returning a [`RunSummary`].
//!
//! The engine is deliberately **pure of any domain or I/O concern**: it depends
//! only on the [`rskit_worker`] bounded pool for concurrency and knows nothing
//! about subprocesses, git, registries, or coverage. All such work lives inside
//! a verb's [`UnitOperation`]. That purity is what makes per-unit streaming safe
//! to parallelize.
#![warn(missing_docs)]

mod engine;
mod gate;
mod graph;
mod lifecycle;
mod operation;

pub use engine::{EngineConfig, execute};
pub use gate::{Gate, UnitState};
pub use graph::{UnitSpec, level_waves};
pub use lifecycle::{Progress, RunSummary, UnitReport, UnitStatus};
pub use operation::{Completed, UnitOperation};
