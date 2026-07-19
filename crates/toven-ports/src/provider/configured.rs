//! The config-bearing adapter instance produced by [`Provider::configure`].

use rskit_errors::AppResult;

use crate::{
    config::{CommonEcosystemConfig, RunStrategy},
    discover::{DiscoverRequest, DiscoverResponse},
    release::ReleaseTarget,
    task::{TaskKind, ToolchainProbe},
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

    /// The default wave-ordering policy for `kind` (ecosystem override
    /// applied).
    fn run_strategy_default(&self, kind: TaskKind) -> RunStrategy;

    /// The release target for this ecosystem, or `None` when not publishable
    /// (`publish = false` / no registry).
    fn release_target(&self) -> AppResult<Option<Box<dyn ReleaseTarget>>>;

    /// The resolved engine-common knobs (`run_strategy`, `release`, `tasks`).
    fn common(&self) -> &CommonEcosystemConfig;
}
