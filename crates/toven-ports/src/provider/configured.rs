//! The config-bearing adapter instance produced by [`Provider::configure`].

use rskit_errors::AppResult;

use crate::{
    VcsReader,
    config::{CommonEcosystemConfig, RunStrategy},
    discover::{DiscoverRequest, DiscoverResponse},
    release::ReleaseAdapter,
    task::{TaskIntent, TaskKind, ToolchainProbe},
};

/// A configured ecosystem adapter — the baked config plus its resolved
/// defaults.
///
/// Produced on demand by [`Provider::configure`](super::Provider::configure);
/// held as `dyn ConfiguredAdapter`. Every method is config-resolution or a
/// query over already-parsed config — discovery-derived knobs (toolchain
/// `version`, resource groups) are resolved later by the planner, not here.
pub trait ConfiguredAdapter {
    /// Discover this ecosystem's modules, edges, and workspaces.
    fn discover(&self, request: &DiscoverRequest) -> AppResult<DiscoverResponse>;

    /// The probe spec the planner runs once per active workspace to compose the
    /// toolchain version identity.
    fn toolchain_probe(&self) -> ToolchainProbe;

    /// The toolchain probes the planner runs for a workspace when the run
    /// targets `intent`, scoped to the tools that specific task actually needs.
    ///
    /// Defaults to the single ecosystem-wide [`toolchain_probe`](Self::toolchain_probe),
    /// which is correct for tooling-backed adapters whose whole workspace shares
    /// one toolchain (one `cargo`, one `go`). The escape-hatch **command**
    /// adapter overrides this: its tasks are heterogeneous tools sharing one
    /// declared workspace, so it returns the probe(s) for the addressed task
    /// (e.g. `ast-grep` for `structure`, `mdbook` for `docs-build`) rather than
    /// probing every command tool on every run. Returning an empty vector means
    /// "no toolchain to probe"; the planner then keeps the workspace's existing
    /// toolchain tag unversioned.
    fn toolchain_probes_for(&self, intent: &TaskIntent) -> Vec<ToolchainProbe> {
        let _ = intent;
        vec![self.toolchain_probe()]
    }

    /// The default wave-ordering policy for `kind` (ecosystem override
    /// applied).
    fn run_strategy_default(&self, kind: TaskKind) -> RunStrategy;

    /// Environment variable names that carry a per-process CPU-parallelism
    /// budget for this ecosystem's fanned-out tools.
    ///
    /// The engine sizes a total compute budget and divides it across the units
    /// running concurrently (see
    /// [`ComputeBudget`](crate::config::ComputeBudget)); for each name returned
    /// here it sets that per-process share in the child's environment, never on
    /// argv. Returning an empty vector (the default) opts the ecosystem out —
    /// correct for a self-balancing single-invocation toolchain (one `cargo
    /// build` already parallelizes internally across all its targets) and for
    /// the escape-hatch **command** ecosystem, whose heterogeneous tools expose
    /// no common parallelism knob. An ecosystem that fans out one CPU-bound
    /// process per module (e.g. Go's `GOMAXPROCS`) overrides this.
    fn compute_budget_env(&self) -> Vec<String> {
        Vec::new()
    }

    /// The ecosystem release adapter when the adapter supports release
    /// mechanics — the composed per-phase seam
    /// ([`ReleaseAdapter`]). Publication policy (`registry`, tag-only, excluded)
    /// and per-phase backing (native or delegated) are resolved by the engine
    /// from configuration.
    fn release_target(&self, reader: &dyn VcsReader) -> AppResult<Option<Box<dyn ReleaseAdapter>>>;

    /// The resolved engine-common knobs (`run_strategy`, `release`, `tasks`).
    fn common(&self) -> &CommonEcosystemConfig;
}
