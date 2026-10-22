//! The config-bearing adapter instance produced by [`Provider::configure`].

use rskit_errors::AppResult;
use toven_model::{AbsPath, ToolchainTag};

use crate::{
    VcsReader,
    config::{CommonEcosystemConfig, RunStrategy},
    discover::{DiscoverRequest, DiscoverResponse},
    provider::DoctorScope,
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

    /// The ecosystem-wide default probe: the tool every workspace of this
    /// ecosystem runs unless [`toolchain_probes_for`](Self::toolchain_probes_for)
    /// narrows it per workspace and task. `doctor` audits it when no workspace
    /// is known.
    fn toolchain_probe(&self) -> ToolchainProbe;

    /// The toolchain probes the planner runs for one workspace when the run
    /// targets `intent`, scoped to the tools that workspace's task actually
    /// needs.
    ///
    /// `toolchain` is the workspace's discovered [`ToolchainTag`]: its `tool`
    /// names the driver discovery chose for that workspace. Tooling adapters
    /// whose every workspace shares one toolchain (one `cargo`, one `go`)
    /// ignore it and return the ecosystem-wide
    /// [`toolchain_probe`](Self::toolchain_probe), which is the default. An
    /// adapter whose workspaces use different drivers (a `js` root on `pnpm`
    /// next to one on `bun`) probes that workspace's driver instead. The
    /// escape-hatch **command** adapter returns the probe for the addressed
    /// task's own tool (e.g. `ast-grep` for `structure`). Returning an empty
    /// vector means "no toolchain to probe"; the planner then keeps the
    /// workspace's toolchain tag unversioned.
    ///
    /// # Errors
    /// An adapter that answers remotely (a federation driver) propagates its
    /// transport or remote failure.
    fn toolchain_probes_for(
        &self,
        toolchain: &ToolchainTag,
        intent: &TaskIntent,
    ) -> AppResult<Vec<ToolchainProbe>> {
        let _ = (toolchain, intent);
        Ok(vec![self.toolchain_probe()])
    }

    /// What `toven doctor` audits for this ecosystem in the repository at
    /// `project_root`: the toolchains to probe and the ecosystem's health checks.
    ///
    /// The default audits the ecosystem-wide
    /// [`toolchain_probe`](Self::toolchain_probe) program with no checks. An
    /// adapter whose workspaces use different drivers returns the drivers this
    /// repository actually uses, and an adapter can add offline checks (an
    /// install that is missing or older than its lockfile).
    ///
    /// # Errors
    /// The adapter cannot read the repository, or a federation driver fails.
    fn doctor(&self, project_root: &AbsPath) -> AppResult<DoctorScope> {
        let _ = project_root;
        Ok(DoctorScope::new(vec![ToolchainTag::new(
            self.toolchain_probe().program,
        )]))
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
