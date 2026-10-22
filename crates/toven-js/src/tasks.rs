//! Per-kind default wave-ordering policy for JavaScript tasks.

use toven_ports::{RunStrategy, TaskKind};

/// The per-kind default wave-ordering policy.
///
/// `build`/`check`/`doc`/`run` keep dependency order: a package usually
/// consumes its workspace dependencies' build output (`dist/`, generated
/// types), so dependencies must build first. `format`/`lint`/`vuln`/`test`/
/// `coverage` collapse into one wave. A test that imports a dependency's build
/// output gets its ordering from `depends_on = ["^build"]`, which onboarding
/// authors, not from a wave order.
#[must_use]
pub(crate) const fn default_run_strategy(kind: TaskKind) -> RunStrategy {
    match kind {
        TaskKind::Format
        | TaskKind::Lint
        | TaskKind::Vuln
        | TaskKind::Test
        | TaskKind::Coverage => RunStrategy::Unordered,
        _ => RunStrategy::LeafToTop,
    }
}

#[cfg(test)]
mod tests {
    use toven_ports::{RunStrategy, TaskKind};

    use super::default_run_strategy;

    #[test]
    fn run_strategy_defaults_by_kind() {
        for kind in [TaskKind::Build, TaskKind::Check] {
            assert_eq!(default_run_strategy(kind), RunStrategy::LeafToTop);
        }
        for kind in [
            TaskKind::Format,
            TaskKind::Lint,
            TaskKind::Vuln,
            TaskKind::Test,
            TaskKind::Coverage,
        ] {
            assert_eq!(default_run_strategy(kind), RunStrategy::Unordered);
        }
    }
}
