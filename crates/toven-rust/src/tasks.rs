//! Per-kind default wave-ordering policy for cargo tasks.
//!
//! The cargo command templates live in [`render`](crate::render) (authored into
//! `toven.toml` at `toven init` time), because the config task table is the
//! authoritative source of runnable tasks. This module owns only the ordering
//! policy the adapter resolves at runtime.

use toven_ports::{RunStrategy, TaskKind};

/// The per-kind default wave-ordering policy.
///
/// Every cargo kind collapses into a single wave. One cargo invocation resolves
/// and compiles its path dependencies itself in dependency order (and stops
/// dependents of a failed crate), including path dependencies that live in a
/// sibling workspace, so a Toven dependency barrier between cargo units only
/// serialises otherwise-independent work.
///
/// `run` keeps dependency order so dependencies start first, and a plain
/// `default` task keeps it because its argv has no known ordering of its own.
#[must_use]
pub(crate) const fn default_run_strategy(kind: TaskKind) -> RunStrategy {
    match kind {
        TaskKind::Run | TaskKind::Default => RunStrategy::LeafToTop,
        _ => RunStrategy::Unordered,
    }
}

#[cfg(test)]
mod tests {
    use toven_ports::{RunStrategy, TaskKind};

    use super::default_run_strategy;

    #[test]
    fn cargo_kinds_collapse_into_one_wave() {
        for kind in [
            TaskKind::Build,
            TaskKind::Check,
            TaskKind::Format,
            TaskKind::Lint,
            TaskKind::Test,
            TaskKind::Coverage,
            TaskKind::Doc,
            TaskKind::Vuln,
        ] {
            assert_eq!(
                default_run_strategy(kind),
                RunStrategy::Unordered,
                "{kind:?}"
            );
        }
    }

    #[test]
    fn run_and_plain_tasks_keep_dependency_order() {
        assert_eq!(default_run_strategy(TaskKind::Run), RunStrategy::LeafToTop);
        assert_eq!(
            default_run_strategy(TaskKind::Default),
            RunStrategy::LeafToTop
        );
    }
}
