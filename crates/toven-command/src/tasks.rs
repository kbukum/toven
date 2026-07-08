//! Per-kind default wave-ordering policy for command tasks.
//!
//! The command adapter has no built-in task templates: the user's
//! `[ecosystems.command.tasks.*]` table is the authoritative source of runnable
//! tasks. This module owns only the ordering policy the adapter resolves at
//! runtime.

use toven_ports::{RunStrategy, TaskKind};

/// The per-kind default wave-ordering policy.
///
/// Declared `depends_on` edges are honored by default (`LeafToTop`);
/// `format`/`lint` are independent and collapse into one wave. The user
/// overrides via `run_strategy`.
#[must_use]
pub(crate) const fn default_run_strategy(kind: TaskKind) -> RunStrategy {
    match kind {
        TaskKind::Format | TaskKind::Lint => RunStrategy::Unordered,
        _ => RunStrategy::LeafToTop,
    }
}

#[cfg(test)]
mod tests {
    use toven_ports::{RunStrategy, TaskKind};

    use super::default_run_strategy;

    #[test]
    fn run_strategy_defaults_by_kind() {
        assert_eq!(
            default_run_strategy(TaskKind::Build),
            RunStrategy::LeafToTop
        );
        assert_eq!(
            default_run_strategy(TaskKind::Format),
            RunStrategy::Unordered
        );
    }
}
