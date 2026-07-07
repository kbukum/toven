//! [`RustAdapter`] — the configured cargo adapter the engine drives.

use rskit_errors::AppResult;
use toven_ports::{
    CommonEcosystemConfig, ConfiguredAdapter, DiscoverRequest, DiscoverResponse, ReleaseTarget,
    RunStrategy, TaskKind, ToolchainProbe,
};

use crate::config::RustConfig;
use crate::discovery;
use crate::release::CratesIoTarget;
use crate::tasks;
use crate::toolchain;

/// The configured cargo adapter: a baked [`RustConfig`].
///
/// Constructed by [`RustProvider::configure`](toven_ports::Provider::configure)
/// and held by the engine as `dyn ConfiguredAdapter`. The runnable task table is
/// read from the parsed config (`common().tasks`), not from the adapter.
#[derive(Debug, Clone)]
pub struct RustAdapter {
    config: RustConfig,
}

impl RustAdapter {
    /// Construct an adapter from a baked config.
    #[must_use]
    pub const fn new(config: RustConfig) -> Self {
        Self { config }
    }
}

impl ConfiguredAdapter for RustAdapter {
    fn discover(&self, request: &DiscoverRequest) -> AppResult<DiscoverResponse> {
        discovery::discover(&self.config, request)
    }

    fn toolchain_probe(&self) -> ToolchainProbe {
        toolchain::cargo_probe()
    }

    fn run_strategy_default(&self, kind: &TaskKind) -> RunStrategy {
        self.config
            .common
            .run_strategy
            .unwrap_or_else(|| tasks::default_run_strategy(kind))
    }

    fn release_target(&self) -> AppResult<Option<Box<dyn ReleaseTarget>>> {
        if self.config.publish {
            Ok(Some(Box::new(CratesIoTarget::new())))
        } else {
            Ok(None)
        }
    }

    fn common(&self) -> &CommonEcosystemConfig {
        &self.config.common
    }
}
