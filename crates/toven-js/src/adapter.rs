//! [`JsAdapter`] — the configured `js` adapter the engine drives.

use std::sync::Arc;

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_model::{AbsPath, ToolchainTag};
use toven_ports::{
    CommonEcosystemConfig, ConfiguredAdapter, DiscoverRequest, DiscoverResponse, DoctorScope,
    ReleaseAdapter, RunStrategy, TaskIntent, TaskKind, ToolRunner, ToolchainProbe, VcsReader,
};

use crate::config::JsConfig;
use crate::discovery;
use crate::doctor;
use crate::release::NpmRegistryTarget;
use crate::tasks;
use crate::toolchain;

/// The configured `js` adapter: a baked [`JsConfig`] and the tool runner its
/// release target uses.
///
/// Discovery reads manifests in-process and never uses the runner.
#[derive(Clone)]
pub struct JsAdapter {
    config: JsConfig,
    runner: Arc<dyn ToolRunner>,
}

impl JsAdapter {
    /// Construct an adapter from a baked config.
    #[must_use]
    pub fn new(config: JsConfig, runner: Arc<dyn ToolRunner>) -> Self {
        Self { config, runner }
    }
}

impl ConfiguredAdapter for JsAdapter {
    fn discover(&self, request: &DiscoverRequest) -> AppResult<DiscoverResponse> {
        discovery::discover(&self.config, request)
    }

    fn toolchain_probe(&self) -> ToolchainProbe {
        toolchain::node_probe()
    }

    fn toolchain_probes_for(
        &self,
        toolchain: &ToolchainTag,
        _intent: &TaskIntent,
    ) -> AppResult<Vec<ToolchainProbe>> {
        toolchain::probes_for(toolchain)
    }

    fn doctor(&self, project_root: &AbsPath) -> AppResult<DoctorScope> {
        doctor::scope(&self.config, project_root.as_path())
    }

    fn run_strategy_default(&self, kind: TaskKind) -> RunStrategy {
        self.config
            .common
            .run_strategy
            .unwrap_or_else(|| tasks::default_run_strategy(kind))
    }

    fn release_target(
        &self,
        _reader: &dyn VcsReader,
    ) -> AppResult<Option<Box<dyn ReleaseAdapter>>> {
        // The engine runs from the repository root, which is the trust
        // boundary for manifest paths.
        let project_root = std::env::current_dir().map_err(|error| {
            AppError::new(ErrorCode::Internal, "failed to read current directory").with_cause(error)
        })?;
        Ok(Some(Box::new(NpmRegistryTarget::new(
            self.config.clone(),
            self.runner.clone(),
            project_root,
        ))))
    }

    fn common(&self) -> &CommonEcosystemConfig {
        &self.config.common
    }
}
