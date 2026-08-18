//! Per-kind default wave-ordering policy for Go tasks.
//!
//! The Go command templates live in [`render`](crate::render) (authored into
//! `toven.toml` at `toven init` time), because the config task table is the
//! authoritative source of runnable tasks. This module owns only the ordering
//! policy the adapter resolves at runtime.

use toven_ports::{RunStrategy, TaskKind};

/// The per-kind default wave-ordering policy.
///
/// Only `build`/`check` keep dependency ordering, and solely for **cascade
/// fail-fast**: when a base module fails to compile every dependent would fail
/// too, so skipping them surfaces the root cause instead of a wall of derived
/// errors. `doc`/`run` keep it for the same compile-first / start-dependencies-
/// first reason.
///
/// Everything else collapses into a single wave. `format`/`lint`/`vuln` are
/// self-evidently independent, and so are `test`/`coverage`: a Go module's `go
/// test ./...` compiles the packages it imports from other workspace modules on
/// demand through Go's shared, content-addressed, concurrency-safe build cache,
/// so it never needs another module's *tests* to have run first. Keeping the
/// build-order edge for a test run is a spurious barrier that only serialises
/// otherwise-independent work (measured ~2x slower under `-race`), and CI wants
/// every module's failures reported, not a fail-fast skip of dependents.
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
        // Only build/check keep dependency ordering (cascade fail-fast on a
        // compile break).
        assert_eq!(
            default_run_strategy(TaskKind::Build),
            RunStrategy::LeafToTop
        );
        assert_eq!(
            default_run_strategy(TaskKind::Check),
            RunStrategy::LeafToTop
        );
        // Independent-per-module kinds collapse into one wave.
        assert_eq!(
            default_run_strategy(TaskKind::Format),
            RunStrategy::Unordered
        );
        assert_eq!(default_run_strategy(TaskKind::Lint), RunStrategy::Unordered);
        assert_eq!(default_run_strategy(TaskKind::Vuln), RunStrategy::Unordered);
        // `go test`/coverage compile dependencies on demand via Go's build
        // cache, so a module's tests never need another module's tests first.
        assert_eq!(default_run_strategy(TaskKind::Test), RunStrategy::Unordered);
        assert_eq!(
            default_run_strategy(TaskKind::Coverage),
            RunStrategy::Unordered
        );
    }
}
