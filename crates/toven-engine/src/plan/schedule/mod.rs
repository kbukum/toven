//! Schedule: relax edges by `RunStrategy`, level into federated waves, group the
//! active modules by intrinsic `FanOut`, and render one [`PlannedUnit`] per group.
//!
//! Per-module `RunStrategy` decides whether a module's **intra-ecosystem** ordering
//! edges are kept (`leaf-to-top`) or dropped (`unordered`); **cross-ecosystem
//! overlay edges are never dropped**. The residual active subgraph is topo-levelled
//! into waves. Modules are then grouped by the task's [`FanOut`]: a `PerModule` task
//! yields one unit per module, while `Batchable`/`WholeWorkspace` tasks collapse all
//! same-ecosystem-and-workspace-and-layer modules into a single invocation. Folding
//! each module's dependency layer into the group id keeps the condensed unit graph
//! acyclic, so a layer-homogeneous group occupies exactly one wave.
//!
//! ## Surface
//! - [`ordering`] — `RunStrategy` relaxation, active-subgraph construction, and the
//!   topo-levelled waves plus the per-module dependency layer derived from them.
//! - [`task`] — resolve each active module's effective [`Task`](toven_ports::Task)
//!   for the intent (adapter default field-merged with any group override).
//! - [`grouping`] — batch-group identity, the dependency-layer fold that keeps the
//!   condensed unit graph acyclic, and the two acyclicity guards.
//! - [`unit`] — render one [`PlannedUnit`] (argv, cache-keying facts, gating edges).
//! - [`entry`] — the [`schedule`] driver that assembles the waves of units.

mod entry;
mod grouping;
mod ordering;
mod task;
mod unit;

#[cfg(test)]
mod tests;

pub(in crate::plan) use entry::{Scheduled, schedule};
