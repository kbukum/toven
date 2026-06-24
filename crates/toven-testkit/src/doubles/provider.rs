//! Shared Provider-side doubles: [`FakeProvider`] and [`FakeConfiguredAdapter`].
//!
//! Planner/discover tests configure canned discovery, tasks, and probe here
//! instead of standing up a real adapter. Both are `Clone` so a [`FakeProvider`]
//! can hand back a fresh boxed adapter from `configure` on each call. The
//! release-target double lives beside this one in [`release`](super::release).

use rskit_errors::AppResult;
use toml::Table;
use toven_model::EcosystemId;
use toven_ports::{
    CommonEcosystemConfig, ConfiguredAdapter, DiscoverRequest, DiscoverResponse, EcosystemFragment,
    Provider, ReleaseTarget, RunStrategy, Task, TaskKind, ToolchainProbe,
};

use super::release::FakeReleaseTarget;

/// A [`ConfiguredAdapter`] returning canned discovery, tasks, and defaults.
///
/// Build it with `with_*`; `discover` returns the scripted response stamped with
/// the request's schema version.
#[derive(Debug, Clone)]
pub struct FakeConfiguredAdapter {
    response: DiscoverResponse,
    tasks: Vec<Task>,
    probe: ToolchainProbe,
    run_strategy: RunStrategy,
    release_target: Option<FakeReleaseTarget>,
    common: CommonEcosystemConfig,
}

impl FakeConfiguredAdapter {
    /// Construct an adapter for `ecosystem` with an empty discovery response,
    /// no tasks, a `cargo --version`-style probe, and `LeafToTop` ordering.
    #[must_use]
    pub fn new(ecosystem: EcosystemId) -> Self {
        Self {
            response: DiscoverResponse::new(ecosystem),
            tasks: Vec::new(),
            probe: ToolchainProbe::new("fake", "fake", vec!["--version".into()]),
            run_strategy: RunStrategy::LeafToTop,
            release_target: None,
            common: CommonEcosystemConfig::default(),
        }
    }

    /// Set the discovery response returned by `discover`.
    #[must_use]
    pub fn with_response(mut self, response: DiscoverResponse) -> Self {
        self.response = response;
        self
    }

    /// Set the adapter's default tasks.
    #[must_use]
    pub fn with_tasks(mut self, tasks: Vec<Task>) -> Self {
        self.tasks = tasks;
        self
    }

    /// Set the toolchain probe spec.
    #[must_use]
    pub fn with_probe(mut self, probe: ToolchainProbe) -> Self {
        self.probe = probe;
        self
    }

    /// Set the default wave-ordering policy.
    #[must_use]
    pub const fn with_run_strategy(mut self, run_strategy: RunStrategy) -> Self {
        self.run_strategy = run_strategy;
        self
    }

    /// Make this ecosystem publishable via the given [`FakeReleaseTarget`].
    #[must_use]
    pub fn with_release_target(mut self, target: FakeReleaseTarget) -> Self {
        self.release_target = Some(target);
        self
    }

    /// Set the resolved engine-common config.
    #[must_use]
    pub fn with_common(mut self, common: CommonEcosystemConfig) -> Self {
        self.common = common;
        self
    }
}

impl ConfiguredAdapter for FakeConfiguredAdapter {
    fn discover(&self, request: &DiscoverRequest) -> AppResult<DiscoverResponse> {
        let mut response = self.response.clone();
        response.schema_version = request.schema_version;
        Ok(response)
    }

    fn default_tasks(&self) -> Vec<Task> {
        self.tasks.clone()
    }

    fn toolchain_probe(&self) -> ToolchainProbe {
        self.probe.clone()
    }

    fn run_strategy_default(&self, _kind: &TaskKind) -> RunStrategy {
        self.run_strategy
    }

    fn release_target(&self) -> AppResult<Option<Box<dyn ReleaseTarget>>> {
        Ok(self
            .release_target
            .clone()
            .map(|target| Box::new(target) as Box<dyn ReleaseTarget>))
    }

    fn common(&self) -> &CommonEcosystemConfig {
        &self.common
    }
}

/// A [`Provider`] that bakes a canned [`FakeConfiguredAdapter`].
///
/// `configure` ignores the raw TOML and returns a clone of the template adapter;
/// `scaffold` returns the scripted fragment (an empty one by default).
#[derive(Debug, Clone)]
pub struct FakeProvider {
    ecosystem: EcosystemId,
    adapter: FakeConfiguredAdapter,
    scaffold: Option<EcosystemFragment>,
}

impl FakeProvider {
    /// Construct a provider for `ecosystem` with a default template adapter and
    /// an empty scaffold fragment.
    #[must_use]
    pub fn new(ecosystem: EcosystemId) -> Self {
        let adapter = FakeConfiguredAdapter::new(ecosystem.clone());
        let scaffold = Some(EcosystemFragment::new(ecosystem.clone(), Table::new()));
        Self {
            ecosystem,
            adapter,
            scaffold,
        }
    }

    /// Set the template adapter `configure` clones on each call.
    #[must_use]
    pub fn with_adapter(mut self, adapter: FakeConfiguredAdapter) -> Self {
        self.adapter = adapter;
        self
    }

    /// Set the fragment returned by `scaffold` (`None` = not present).
    #[must_use]
    pub fn with_scaffold(mut self, scaffold: Option<EcosystemFragment>) -> Self {
        self.scaffold = scaffold;
        self
    }
}

impl Provider for FakeProvider {
    fn ecosystem_id(&self) -> &EcosystemId {
        &self.ecosystem
    }

    fn configure(&self, _raw: toml::Value) -> AppResult<Box<dyn ConfiguredAdapter>> {
        Ok(Box::new(self.adapter.clone()))
    }

    fn scaffold(&self, _project_root: &std::path::Path) -> AppResult<Option<EcosystemFragment>> {
        Ok(self.scaffold.clone())
    }
}

#[cfg(test)]
mod tests {
    use toml::Table;
    use toven_model::{AbsPath, EcosystemId};
    use toven_ports::{DiscoverRequest, Provider};

    use super::super::release::FakeReleaseTarget;
    use super::{FakeConfiguredAdapter, FakeProvider};

    fn rust() -> EcosystemId {
        EcosystemId::new("rust").expect("valid id")
    }

    #[test]
    fn provider_configures_clone_of_template_adapter() {
        let adapter =
            FakeConfiguredAdapter::new(rust()).with_release_target(FakeReleaseTarget::new());
        let provider = FakeProvider::new(rust()).with_adapter(adapter);

        assert_eq!(provider.ecosystem_id().as_str(), "rust");
        let configured = provider
            .configure(toml::Value::Table(Table::new()))
            .expect("configures");

        let request = DiscoverRequest::new(AbsPath::new("/repo").expect("absolute"));
        let response = configured.discover(&request).expect("discovers");
        assert_eq!(response.schema_version, request.schema_version);
        assert!(configured.release_target().expect("ok").is_some());
    }
}
