//! [`GoAdapter`] — the configured `go` adapter the engine drives.

use rskit_errors::AppResult;
use toven_ports::{
    CommonEcosystemConfig, ConfiguredAdapter, DiscoverRequest, DiscoverResponse, ReleaseTarget,
    RunStrategy, TaskKind, ToolchainProbe,
};

use crate::config::GoConfig;
use crate::discovery;
use crate::tasks;
use crate::toolchain;

/// The configured `go` adapter: a baked [`GoConfig`].
///
/// Constructed by [`GoProvider::configure`](toven_ports::Provider::configure)
/// and held by the engine as `dyn ConfiguredAdapter`. The runnable task table is
/// read from the parsed config (`common().tasks`), not from the adapter.
#[derive(Debug, Clone)]
pub struct GoAdapter {
    config: GoConfig,
}

impl GoAdapter {
    /// Construct an adapter from a baked config.
    #[must_use]
    pub const fn new(config: GoConfig) -> Self {
        Self { config }
    }
}

impl ConfiguredAdapter for GoAdapter {
    fn discover(&self, request: &DiscoverRequest) -> AppResult<DiscoverResponse> {
        discovery::discover(&self.config, request)
    }

    fn toolchain_probe(&self) -> ToolchainProbe {
        toolchain::go_probe()
    }

    fn run_strategy_default(&self, kind: &TaskKind) -> RunStrategy {
        self.config
            .common
            .run_strategy
            .unwrap_or_else(|| tasks::default_run_strategy(kind))
    }

    fn release_target(&self) -> AppResult<Option<Box<dyn ReleaseTarget>>> {
        // Go modules do not expose a release target through this adapter.
        Ok(None)
    }

    fn common(&self) -> &CommonEcosystemConfig {
        &self.config.common
    }
}
