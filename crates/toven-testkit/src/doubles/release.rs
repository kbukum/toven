//! Shared [`ReleaseTarget`] double: [`FakeReleaseTarget`].
//!
//! Release-engine tests configure canned version I/O, scripted publish
//! behaviour, and call recording here instead of standing up a real adapter.
//! It is `Clone` so a [`FakeConfiguredAdapter`](super::FakeConfiguredAdapter)
//! can hand back a fresh boxed target from `release_target` on each call.

use std::sync::{Arc, Mutex};

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_version::semver::Version;
use toven_model::{Module, ModuleRef};
use toven_ports::{Artifact, PublishOutcome, ReleaseMutation, ReleaseTarget};

/// A single call recorded by [`FakeReleaseTarget`].
#[derive(Debug, Clone)]
pub enum ReleaseCall {
    /// `declared_version` was called for a module.
    DeclaredVersion(ModuleRef),
    /// `published_versions` was called for a module.
    PublishedVersions(ModuleRef),
    /// `package` was called for a module.
    Package(ModuleRef),
    /// `apply_release` was called with a module mutation.
    ApplyRelease {
        /// Module being mutated.
        module: ModuleRef,
        /// Mutation passed to the target.
        mutation: ReleaseMutation,
    },
    /// `publish` was called for a module.
    Publish(ModuleRef),
}

/// A [`ReleaseTarget`] with canned version I/O, scripted publish behaviour, and
/// call recording.
#[derive(Debug, Clone)]
pub struct FakeReleaseTarget {
    inner: Arc<Mutex<FakeReleaseState>>,
}

#[derive(Debug, Clone)]
struct FakeReleaseState {
    declared: Version,
    published: Vec<Version>,
    artifact_path: String,
    outcomes: Vec<PublishOutcome>,
    publish_index: usize,
    fail_apply: Option<String>,
    fail_package: Option<String>,
    fail_publish: Option<String>,
    calls: Vec<ReleaseCall>,
}

impl Default for FakeReleaseTarget {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(FakeReleaseState {
                declared: Version::new(0, 1, 0),
                published: Vec::new(),
                artifact_path: "dist/fake.pkg".to_string(),
                outcomes: vec![PublishOutcome::Published],
                publish_index: 0,
                fail_apply: None,
                fail_package: None,
                fail_publish: None,
                calls: Vec::new(),
            })),
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
    pub fn with_declared_version(self, version: Version) -> Self {
        self.state().declared = version;
        self
    }

    /// Set the versions the registry reports as already published.
    #[must_use]
    pub fn with_published_versions(self, versions: Vec<Version>) -> Self {
        self.state().published = versions;
        self
    }

    /// Set the classified outcome returned by `publish`.
    #[must_use]
    pub fn with_publish_outcome(self, outcome: PublishOutcome) -> Self {
        self.state().outcomes = vec![outcome];
        self
    }

    /// Set the classified outcomes returned by consecutive `publish` calls.
    #[must_use]
    pub fn with_publish_outcomes(self, outcomes: Vec<PublishOutcome>) -> Self {
        let mut state = self.state();
        state.outcomes = if outcomes.is_empty() {
            vec![PublishOutcome::Published]
        } else {
            outcomes
        };
        state.publish_index = 0;
        drop(state);
        self
    }

    /// Make `apply_release` fail with a typed internal error.
    #[must_use]
    pub fn with_apply_failure(self, message: impl Into<String>) -> Self {
        self.state().fail_apply = Some(message.into());
        self
    }

    /// Make `package` fail with a typed internal error.
    #[must_use]
    pub fn with_package_failure(self, message: impl Into<String>) -> Self {
        self.state().fail_package = Some(message.into());
        self
    }

    /// Make `publish` fail with a typed internal error before returning outcomes.
    #[must_use]
    pub fn with_publish_failure(self, message: impl Into<String>) -> Self {
        self.state().fail_publish = Some(message.into());
        self
    }

    /// Snapshot the recorded release calls in call order.
    #[must_use]
    pub fn calls(&self) -> Vec<ReleaseCall> {
        self.state().calls.clone()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, FakeReleaseState> {
        self.inner.lock().expect("FakeReleaseTarget mutex poisoned")
    }

    fn record(&self, call: ReleaseCall) {
        self.state().calls.push(call);
    }
}

impl ReleaseTarget for FakeReleaseTarget {
    fn declared_version(&self, module: &Module) -> AppResult<Version> {
        self.record(ReleaseCall::DeclaredVersion(module.id.clone()));
        Ok(self.state().declared.clone())
    }

    fn published_versions(&self, module: &Module) -> AppResult<Vec<Version>> {
        self.record(ReleaseCall::PublishedVersions(module.id.clone()));
        Ok(self.state().published.clone())
    }

    fn package(&self, module: &Module) -> AppResult<Artifact> {
        self.record(ReleaseCall::Package(module.id.clone()));
        let state = self.state();
        if let Some(message) = &state.fail_package {
            return Err(fake_error(message));
        }
        Ok(Artifact::new(&state.artifact_path))
    }

    fn apply_release(&self, module: &Module, mutation: &ReleaseMutation) -> AppResult<()> {
        self.record(ReleaseCall::ApplyRelease {
            module: module.id.clone(),
            mutation: mutation.clone(),
        });
        if let Some(message) = &self.state().fail_apply {
            return Err(fake_error(message));
        }
        Ok(())
    }

    fn publish(&self, module: &Module, _artifact: &Artifact) -> AppResult<PublishOutcome> {
        self.record(ReleaseCall::Publish(module.id.clone()));
        let mut state = self.state();
        if let Some(message) = &state.fail_publish {
            return Err(fake_error(message));
        }
        let outcome = state
            .outcomes
            .get(state.publish_index)
            .or_else(|| state.outcomes.last())
            .cloned()
            .unwrap_or(PublishOutcome::Published);
        state.publish_index = state.publish_index.saturating_add(1);
        drop(state);
        Ok(outcome)
    }
}

fn fake_error(message: &str) -> AppError {
    AppError::new(ErrorCode::Internal, message.to_string())
}

#[cfg(test)]
mod tests {
    use rskit_version::semver::Version;
    use toven_model::{EcosystemId, Module, ModuleRef, RepoPath};
    use toven_ports::{Artifact, PublishOutcome, ReleaseTarget};

    use super::FakeReleaseTarget;

    fn module() -> Module {
        Module::new(
            ModuleRef::new(EcosystemId::new("rust").expect("valid id"), "errors")
                .expect("valid ref"),
            RepoPath::new("crates/errors").expect("valid path"),
        )
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
