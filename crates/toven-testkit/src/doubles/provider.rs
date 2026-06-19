//! Shared Provider-side doubles: [`FakeProvider`], [`FakeConfiguredAdapter`],
//! and [`FakeReleaseTarget`].
//!
//! Planner/discover/release tests configure canned discovery, tasks, probe, and
//! release behaviour here instead of standing up a real adapter. All three are
//! `Clone` so a [`FakeProvider`] can hand back a fresh boxed adapter from
//! `configure` on each call.

use rskit_errors::AppResult;
use rskit_version::semver::Version;
use toml::Table;
use toven_model::{EcosystemId, Module};
use toven_ports::{
    Artifact, CommonEcosystemConfig, ConfiguredAdapter, DiscoverRequest, DiscoverResponse,
    EcosystemFragment, Provider, PublishOutcome, ReleaseMutation, ReleaseTarget, RunStrategy, Task,
    TaskKind, ToolchainProbe,
};

/// A [`ReleaseTarget`] with canned version I/O and publish behaviour.
///
/// `apply_release` and `publish` are no-ops that succeed; planning/idempotency
/// tests drive ordering and the bump plan in the engine, which owns them.
#[derive(Debug, Clone)]
pub struct FakeReleaseTarget {
    declared: Version,
    published: Vec<Version>,
    artifact_path: String,
    outcome: PublishOutcome,
}

impl Default for FakeReleaseTarget {
    fn default() -> Self {
        Self {
            declared: Version::new(0, 1, 0),
            published: Vec::new(),
            artifact_path: "dist/fake.pkg".to_string(),
            outcome: PublishOutcome::Published,
        }
    }
}

impl FakeReleaseTarget {
    /// Construct a target declaring `0.1.0` with nothing published.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the version read from the manifest.
    #[must_use]
    pub fn with_declared_version(mut self, version: Version) -> Self {
        self.declared = version;
        self
    }

    /// Set the versions the registry reports as already published.
    #[must_use]
    pub fn with_published_versions(mut self, versions: Vec<Version>) -> Self {
        self.published = versions;
        self
    }

    /// Set the classified outcome returned by `publish`.
    #[must_use]
    pub const fn with_publish_outcome(mut self, outcome: PublishOutcome) -> Self {
        self.outcome = outcome;
        self
    }
}

impl ReleaseTarget for FakeReleaseTarget {
    fn declared_version(&self, _module: &Module) -> AppResult<Version> {
        Ok(self.declared.clone())
    }

    fn published_versions(&self, _module: &Module) -> AppResult<Vec<Version>> {
        Ok(self.published.clone())
    }

    fn package(&self, _module: &Module) -> AppResult<Artifact> {
        Ok(Artifact::new(&self.artifact_path))
    }

    fn apply_release(&self, _module: &Module, _mutation: &ReleaseMutation) -> AppResult<()> {
        Ok(())
    }

    fn publish(&self, _module: &Module, _artifact: &Artifact) -> AppResult<PublishOutcome> {
        Ok(self.outcome.clone())
    }
}

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
    use rskit_version::semver::Version;
    use toml::Table;
    use toven_model::{AbsPath, EcosystemId, Module, ModuleRef, RepoPath};
    use toven_ports::{Artifact, DiscoverRequest, Provider, PublishOutcome, ReleaseTarget};

    use super::{FakeConfiguredAdapter, FakeProvider, FakeReleaseTarget};

    fn rust() -> EcosystemId {
        EcosystemId::new("rust").expect("valid id")
    }

    fn module() -> Module {
        Module::new(
            ModuleRef::new(rust(), "errors").expect("valid ref"),
            RepoPath::new("crates/errors").expect("valid path"),
        )
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

    #[test]
    fn release_target_returns_scripted_outcome() {
        let target = FakeReleaseTarget::new()
            .with_declared_version(Version::new(1, 2, 3))
            .with_publish_outcome(PublishOutcome::AlreadyPublished);
        let module = module();

        assert_eq!(
            target.declared_version(&module).expect("ok"),
            Version::new(1, 2, 3)
        );
        assert_eq!(
            target
                .publish(&module, &Artifact::new("dist/x"))
                .expect("ok"),
            PublishOutcome::AlreadyPublished
        );
    }
}
