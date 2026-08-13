//! [`GoAdapter`] — the configured `go` adapter the engine drives.

use std::sync::Arc;

use rskit_errors::AppResult;
use toven_ports::{
    CommonEcosystemConfig, ConfiguredAdapter, DiscoverRequest, DiscoverResponse, ReleaseAdapter,
    RunStrategy, TaskKind, ToolRunner, ToolchainProbe, VcsReader,
};

use crate::config::GoConfig;
use crate::discovery;
use crate::release::GoVcsTarget;
use crate::tasks;
use crate::toolchain;

/// The configured `go` adapter: a baked [`GoConfig`].
///
/// Constructed by [`GoProvider::configure`](toven_ports::Provider::configure)
/// and held by the engine as `dyn ConfiguredAdapter`. The runnable task table
/// is read from the parsed config (`common().tasks`), not from the adapter.
#[derive(Clone)]
pub struct GoAdapter {
    config: GoConfig,
    runner: Arc<dyn ToolRunner>,
}

impl GoAdapter {
    /// Construct an adapter from a baked config.
    #[must_use]
    pub fn new(config: GoConfig, runner: Arc<dyn ToolRunner>) -> Self {
        Self { config, runner }
    }
}

impl ConfiguredAdapter for GoAdapter {
    fn discover(&self, request: &DiscoverRequest) -> AppResult<DiscoverResponse> {
        discovery::discover(&self.config, request, self.runner.as_ref())
    }

    fn toolchain_probe(&self) -> ToolchainProbe {
        toolchain::go_probe()
    }

    fn run_strategy_default(&self, kind: TaskKind) -> RunStrategy {
        self.config
            .common
            .run_strategy
            .unwrap_or_else(|| tasks::default_run_strategy(kind))
    }

    fn release_target(&self, reader: &dyn VcsReader) -> AppResult<Option<Box<dyn ReleaseAdapter>>> {
        Ok(Some(Box::new(GoVcsTarget::new(
            self.runner.clone(),
            super::release::reachable_tags(reader)?,
        ))))
    }

    fn common(&self) -> &CommonEcosystemConfig {
        &self.config.common
    }
}
