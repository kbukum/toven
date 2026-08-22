//! [`ApplyOptions`] — runtime knobs for the APPLY spine.

use std::time::Duration;

use toven_model::EcosystemScope;
use toven_ports::{ComputeBudget, InvocationEnvironment};

/// Default bound on the live raw-output bridge (see
/// [`ApplyOptions::live_output_capacity`]). Generous enough to absorb ordinary
/// bursts without backpressuring well-behaved processes, small enough to keep
/// the bridge bounded under a persistently slow consumer.
const DEFAULT_LIVE_OUTPUT_CAPACITY: usize = 1024;

/// Runtime options for APPLY.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ApplyOptions {
    /// Maximum units executing concurrently.
    pub max_parallel: usize,
    /// Cancel in-flight work and stop scheduling after the first failure.
    pub fail_fast: bool,
    /// Environment policy used for every task command.
    ///
    /// Defaults to inheriting the parent process environment so spawned
    /// toolchains (cargo, go, git, …) see the variables they rely on — `HOME`,
    /// `CARGO_HOME`, `RUSTUP_HOME`, `GOPATH`, `SSH_AUTH_SOCK`, locale, proxies,
    /// and so on. Embedders wanting a hermetic run can override this with an
    /// explicit [`InvocationEnvironment`].
    pub environment: InvocationEnvironment,
    /// Bound on the live raw-output bridge between persistent process reader
    /// threads and the APPLY consumer.
    ///
    /// Persistent units can emit output indefinitely; a bounded channel keeps
    /// the bridge from growing without limit when the consumer (e.g. a slow
    /// [`RawOutputSink`](toven_ports::RawOutputSink)) falls behind. When the
    /// channel is full the producing reader thread blocks, applying
    /// backpressure down to the child process's pipe rather than buffering or
    /// dropping output. The consumer drains the bridge continuously while units
    /// run and concurrently during teardown, so a full channel never deadlocks
    /// a process awaiting shutdown.
    pub live_output_capacity: usize,
    /// Optional wall-clock bound on any single normal (non-persistent) unit.
    ///
    /// When set, a unit that runs longer than this is cooperatively cancelled
    /// (the same SIGTERM-and-wait teardown as `--fail-fast`/Ctrl+C) and
    /// recorded as [`UnitStatus::TimedOut`](toven_model::UnitStatus::TimedOut)
    /// — a failure — rather than being allowed to run unbounded. `None` (the
    /// default) leaves normal units unbounded. Persistent units are governed by
    /// their own [`readiness_timeout`](toven_model::ExecutionUnit::readiness_timeout)
    /// probe, not this bound.
    pub unit_timeout: Option<Duration>,
    /// CPU-parallelism budget divided across the units running concurrently in
    /// a wave and injected into each fanned-out tool (see
    /// [`ComputeBudget`]). This is the global default; per-scope overrides
    /// live in [`ecosystem_budgets`]. Only scopes present in [`budget_env`]
    /// receive an injection.
    ///
    /// [`ecosystem_budgets`]: Self::ecosystem_budgets
    /// [`budget_env`]: Self::budget_env
    pub compute_budget: ComputeBudget,
    /// Per-scope [`compute_budget`] overrides
    /// (`[ecosystems.<id>].compute_budget`), keyed by
    /// [`EcosystemScope`] so a cross-repo umbrella can pin two members' shared
    /// ecosystem (`go`) independently; a scope absent here uses the global
    /// [`compute_budget`].
    ///
    /// [`compute_budget`]: Self::compute_budget
    pub ecosystem_budgets: std::collections::BTreeMap<EcosystemScope, ComputeBudget>,
    /// Per-scope environment-variable names that carry a fanned-out tool's
    /// share of the [`compute_budget`]. Built by the CLI from each configured
    /// adapter's
    /// [`compute_budget_env`](toven_ports::ConfiguredAdapter::compute_budget_env),
    /// keyed by [`EcosystemScope`] so per-member config is not collapsed; a
    /// scope absent here (or mapped to an empty list) is never injected, so the
    /// default (empty) preserves today's behavior.
    ///
    /// [`compute_budget`]: Self::compute_budget
    pub budget_env: std::collections::BTreeMap<EcosystemScope, Vec<String>>,
}

impl Default for ApplyOptions {
    fn default() -> Self {
        Self {
            max_parallel: std::thread::available_parallelism().map_or(1, std::num::NonZero::get),
            fail_fast: false,
            environment: InvocationEnvironment::inherit_parent(std::collections::BTreeMap::new()),
            live_output_capacity: DEFAULT_LIVE_OUTPUT_CAPACITY,
            unit_timeout: None,
            compute_budget: ComputeBudget::default(),
            ecosystem_budgets: std::collections::BTreeMap::new(),
            budget_env: std::collections::BTreeMap::new(),
        }
    }
}

/// Whether normal-unit output should stream live for this run.
///
/// Live streaming a normal unit's output is safe only when concurrent chunks
/// cannot intermix on the same lines. That holds in two cases:
///
/// - the sink de-interleaves output spatially (`sink_concurrent_live` — one
///   visual region per `unit_id`), so any number of units can stream at once;
///   or
/// - nothing else can emit concurrently on the single linear stream, which
///   requires both serial or single-unit execution (`max_parallel <= 1` or a
///   one-unit plan) and no held persistent unit (a persistent unit keeps
///   emitting live across the later normal-unit waves even under serial
///   execution).
///
/// When neither holds, normal output is buffered into a deterministic per-unit
/// block instead.
pub(super) fn stream_normal_live(
    options: &ApplyOptions,
    plan: &toven_model::Plan,
    sink_concurrent_live: bool,
) -> bool {
    if sink_concurrent_live {
        return true;
    }
    let serial_or_single = options.max_parallel <= 1 || plan.units.len() <= 1;
    let has_persistent = plan.units.iter().any(|unit| unit.persistent);
    serial_or_single && !has_persistent
}
