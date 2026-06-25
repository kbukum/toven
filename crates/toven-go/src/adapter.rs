//! [`GoAdapter`] — the configured `go` adapter the engine drives.

use rskit_errors::AppResult;
use toven_ports::{
    CommonEcosystemConfig, ConfiguredAdapter, DiscoverRequest, DiscoverResponse, ReleaseTarget,
    RunStrategy, Task, TaskKind, ToolchainProbe,
};

use crate::config::GoConfig;
use crate::discovery;
use crate::tasks;
use crate::toolchain;

/// The configured `go` adapter: a baked [`GoConfig`] plus its resolved task
/// table.
///
/// Constructed by [`GoProvider::configure`](toven_ports::Provider::configure)
/// and held by the engine as `dyn ConfiguredAdapter`.
#[derive(Debug, Clone)]
pub struct GoAdapter {
    config: GoConfig,
    tasks: Vec<Task>,
}

impl GoAdapter {
    /// Construct an adapter from a baked config and its resolved tasks.
    #[must_use]
    pub const fn new(config: GoConfig, tasks: Vec<Task>) -> Self {
        Self { config, tasks }
    }
}

impl ConfiguredAdapter for GoAdapter {
    fn discover(&self, request: &DiscoverRequest) -> AppResult<DiscoverResponse> {
        discovery::discover(&self.config, request)
    }

    fn default_tasks(&self) -> Vec<Task> {
        self.tasks.clone()
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
        // Go-module release is out of scope (step 9.5); the rust crates.io target
        // remains the only release path.
        Ok(None)
    }

    fn common(&self) -> &CommonEcosystemConfig {
        &self.config.common
    }
}
