//! Per-kind default wave-ordering policy for cargo tasks.
//!
//! The cargo command templates live in [`render`](crate::render) (authored into
//! `toven.toml` at `toven init` time), because the config task table is the
//! authoritative source of runnable tasks. This module owns only the ordering
//! policy the adapter resolves at runtime.

use toven_ports::{RunStrategy, TaskKind};

/// The per-kind default wave-ordering policy.
///
/// Compilation-bearing kinds (`build`/`check`/`test`/`doc`/`run`) respect the
/// dependency graph; `format`/`lint` are independent and collapse into one wave.
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
        assert_eq!(
            default_run_strategy(TaskKind::Lint),
            RunStrategy::Unordered
        );
    }
}
