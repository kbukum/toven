//! Fan-out capability — how a ready wave collapses into invocations.

use serde::{Deserialize, Serialize};

/// Intrinsic fan-out capability of a task command.
///
/// This is a **ceiling**, not a scheduling order: the adapter declares what the
/// command supports; the engine decides, within that ceiling, whether to batch
/// or spawn-each. Separate from `RunStrategy` (wave ordering).
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FanOut {
    /// Selector takes exactly one module → one process per module (`go test ./api`).
    PerModule,
    /// Selector is repeatable → engine MAY collapse a ready wave (`cargo test -p a -p b`).
    Batchable,
    /// No selector; runs once per workspace (`cargo fmt --all`).
    WholeWorkspace,
}
