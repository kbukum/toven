//! Shared Provider-side doubles: [`FakeProvider`] and
//! [`FakeConfiguredAdapter`].
//!
//! Planner/discover tests configure canned discovery, tasks, and probe here
//! instead of standing up a real adapter. Wizard tests preset a detection,
//! questionnaire, and rendered fragment. Both are `Clone` so a [`FakeProvider`]
//! can hand back a fresh boxed adapter from `configure` on each call. The
//! release-target double lives beside this one in [`release`](super::release).

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_model::EcosystemId;
use toven_ports::{
    Answers, CommonEcosystemConfig, ConfiguredAdapter, DEFAULT_READINESS_TIMEOUT, Detection,
    DiscoverRequest, DiscoverResponse, EcosystemFragment, Provider, Questionnaire, ReleaseAdapter,
    RunStrategy, Task, TaskEntry, TaskKind, ToolchainProbe,
};

use super::release::FakeReleaseTarget;

/// A [`ConfiguredAdapter`] returning canned discovery, config, and defaults.
///
/// Build it with `with_*`; `discover` returns the scripted response stamped
/// with the request's schema version. The task table lives in the resolved
/// [`CommonEcosystemConfig`] (config is authoritative), so [`with_tasks`] folds
/// resolved [`Task`]s back into the `[ecosystems.<id>.tasks]` config
/// projection.
///
/// [`with_tasks`]: Self::with_tasks
#[derive(Debug, Clone)]
pub struct FakeConfiguredAdapter {
    response: DiscoverResponse,
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

    /// Fold resolved [`Task`]s into the config task table (`common().tasks`),
    /// the authoritative source the engine reads. Each task is projected to its
    /// [`TaskEntry`] keyed by its user-addressable name.
    #[must_use]
    pub fn with_tasks(mut self, tasks: Vec<Task>) -> Self {
        for task in tasks {
            let (key, entry) = task_entry(&task);
            self.common.tasks.insert(key, entry);
        }
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

    /// Set the resolved engine-common config (including its task table).
    #[must_use]
    pub fn with_common(mut self, common: CommonEcosystemConfig) -> Self {
        self.common = common;
        self
    }
}

/// Project a resolved [`Task`] into a `(key, TaskEntry)` config pair — the
/// inverse of [`TaskEntry::materialize`](toven_ports::TaskEntry::materialize).
fn task_entry(task: &Task) -> (String, TaskEntry) {
    let key = task.name.clone();
    // Persist an explicit kind only when it is not the one derived from the name,
    // so a renamed/tagged task round-trips its recognition attribute.
    let derived = TaskKind::from_name(&task.name).unwrap_or(TaskKind::Default);
    let kind = (derived != task.kind).then_some(task.kind);
    let readiness_timeout_secs = (task.readiness_timeout != DEFAULT_READINESS_TIMEOUT)
        .then_some(task.readiness_timeout.as_secs());
    let entry = TaskEntry {
        kind,
        argv: task.argv.clone(),
        selector: task.selector.clone(),
        fan_out: task.fan_out,
        persistent: task.persistent,
        readiness: task.readiness.clone(),
        readiness_timeout_secs,
        cache_args: task.cache_args,
        cacheable: task.cacheable,
        fail_if_output: task.fail_if_output,
        shared_inputs: task.shared_inputs.clone(),
    };
    (key, entry)
}

impl ConfiguredAdapter for FakeConfiguredAdapter {
    fn discover(&self, request: &DiscoverRequest) -> AppResult<DiscoverResponse> {
        let mut response = self.response.clone();
        response.schema_version = request.schema_version;
        Ok(response)
    }

    fn toolchain_probe(&self) -> ToolchainProbe {
        self.probe.clone()
    }

    fn run_strategy_default(&self, _kind: TaskKind) -> RunStrategy {
        self.run_strategy
    }

    fn release_target(
        &self,
        _reader: &dyn toven_ports::VcsReader,
    ) -> AppResult<Option<Box<dyn ReleaseAdapter>>> {
        Ok(self
            .release_target
            .clone()
            .map(|target| Box::new(target) as Box<dyn ReleaseAdapter>))
    }

    fn common(&self) -> &CommonEcosystemConfig {
        &self.common
    }
}

/// A [`Provider`] that bakes a canned [`FakeConfiguredAdapter`] and scripts the
/// three-step wizard (`detect` / `questionnaire` / `render`).
///
/// `configure` ignores the raw TOML and returns a clone of the template
/// adapter; `detect` returns the scripted detection (a bare one by default),
/// `render` returns the scripted fragment (an empty one by default).
#[derive(Debug, Clone)]
pub struct FakeProvider {
    ecosystem: EcosystemId,
    adapter: FakeConfiguredAdapter,
    detection: Option<Detection>,
    detect_error: Option<(ErrorCode, String)>,
    questionnaire: Questionnaire,
    fragment: EcosystemFragment,
    render_error: Option<(ErrorCode, String)>,
}

impl FakeProvider {
    /// Construct a provider for `ecosystem` with a default template adapter, a
    /// bare detection, an empty questionnaire, and an empty rendered fragment.
    #[must_use]
    pub fn new(ecosystem: EcosystemId) -> Self {
        let adapter = FakeConfiguredAdapter::new(ecosystem.clone());
        Self {
            ecosystem: ecosystem.clone(),
            adapter,
            detection: Some(Detection::bare(ecosystem.clone())),
            detect_error: None,
            questionnaire: Questionnaire::empty(ecosystem.clone()),
            fragment: EcosystemFragment::new(ecosystem, toml::Table::new()),
            render_error: None,
        }
    }

    /// Set the template adapter `configure` clones on each call.
    #[must_use]
    pub fn with_adapter(mut self, adapter: FakeConfiguredAdapter) -> Self {
        self.adapter = adapter;
        self
    }

    /// Set the detection returned by `detect` (`None` = ecosystem not present).
    #[must_use]
    pub fn with_detection(mut self, detection: Option<Detection>) -> Self {
        self.detection = detection;
        self
    }

    /// Make `detect` fail with the given typed error (models a driver whose
    /// self-detection itself errors).
    #[must_use]
    pub fn with_detect_error(mut self, code: ErrorCode, message: impl Into<String>) -> Self {
        self.detect_error = Some((code, message.into()));
        self
    }

    /// Set the questionnaire returned by `questionnaire`.
    #[must_use]
    pub fn with_questionnaire(mut self, questionnaire: Questionnaire) -> Self {
        self.questionnaire = questionnaire;
        self
    }

    /// Set the fragment returned by `render`.
    #[must_use]
    pub fn with_fragment(mut self, fragment: EcosystemFragment) -> Self {
        self.fragment = fragment;
        self
    }

    /// Make `render` fail with the given typed error.
    #[must_use]
    pub fn with_render_error(mut self, code: ErrorCode, message: impl Into<String>) -> Self {
        self.render_error = Some((code, message.into()));
        self
    }
}

impl Provider for FakeProvider {
    fn ecosystem_id(&self) -> &EcosystemId {
        &self.ecosystem
    }

    fn configure(&self, _raw: rskit_config::RawValue) -> AppResult<Box<dyn ConfiguredAdapter>> {
        Ok(Box::new(self.adapter.clone()))
    }

    fn detect(&self, _project_root: &std::path::Path) -> AppResult<Option<Detection>> {
        if let Some((code, message)) = &self.detect_error {
            return Err(AppError::new(*code, message.clone()));
        }
        Ok(self.detection.clone())
    }

    fn questionnaire(&self, _detection: &Detection) -> AppResult<Questionnaire> {
        Ok(self.questionnaire.clone())
    }

    fn render(&self, _detection: &Detection, _answers: &Answers) -> AppResult<EcosystemFragment> {
        if let Some((code, message)) = &self.render_error {
            return Err(AppError::new(*code, message.clone()));
        }
        Ok(self.fragment.clone())
    }
}

#[cfg(test)]
mod tests {
    use toven_model::{AbsPath, EcosystemId};
    use toven_ports::{ConfiguredAdapter, DiscoverRequest, FanOut, Provider, Task};

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
            .configure(rskit_config::RawValue::Null)
            .expect("configures");

        let request = DiscoverRequest::new(AbsPath::new("/repo").expect("absolute"));
        let response = configured.discover(&request).expect("discovers");
        assert_eq!(response.schema_version, request.schema_version);
        let reader = crate::doubles::FakeVcsReader::new();
        assert!(configured.release_target(&reader).expect("ok").is_some());
    }

    #[test]
    fn with_tasks_projects_into_the_config_table() {
        let adapter = FakeConfiguredAdapter::new(rust()).with_tasks(vec![Task::new(
            "test",
            vec!["cargo".into(), "test".into()],
            FanOut::Batchable,
        )]);
        let entry = adapter.common().tasks.get("test").expect("test entry");
        assert_eq!(entry.argv, ["cargo", "test"]);
        assert_eq!(entry.fan_out, FanOut::Batchable);
        assert!(entry.kind.is_none());
    }
}
