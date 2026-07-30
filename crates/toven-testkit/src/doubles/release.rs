//! Shared [`ReleaseTarget`] double: [`FakeReleaseTarget`].
//!
//! Release-engine tests configure canned version I/O, scripted publish
//! behaviour, and call recording here instead of standing up a real adapter. It
//! is `Clone` so a [`FakeConfiguredAdapter`](super::FakeConfiguredAdapter) can
//! hand back a fresh boxed target from `release_target` on each call.

use std::path::Path;
use std::sync::{Arc, Mutex};

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_version::semver::Version;
use toven_model::{Module, ModuleRef};
use toven_ports::{
    Artifact, PublishOutcome, ReleaseCredentials, ReleaseMutation, ReleaseTarget, TagScheme,
};

/// A single call recorded by [`FakeReleaseTarget`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ReleaseCall {
    /// `declared_version` was called for a module.
    DeclaredVersion(ModuleRef),
    /// `published_versions` was called for a module.
    PublishedVersions(ModuleRef),
    /// `tag_scheme` was called for a module.
    TagScheme {
        /// Module whose tag scheme was requested.
        module: ModuleRef,
        /// Configured tag format passed by the engine.
        tag_format: Option<String>,
    },
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
    /// `sbom` was called for a module, with the bounded output directory the
    /// engine passed.
    Sbom {
        /// Module the SBOM was requested for.
        module: ModuleRef,
        /// Output directory the tool invocation was bounded to.
        out_dir: String,
    },
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
    tag_scheme: Option<TagScheme>,
    outcomes: Vec<PublishOutcome>,
    publish_index: usize,
    fail_apply: Option<String>,
    fail_package: Option<String>,
    fail_publish: Option<String>,
    fail_version_read: Option<String>,
    sbom_artifact: Option<String>,
    fail_sbom: Option<String>,
    calls: Vec<ReleaseCall>,
    publish_token_envs: Vec<Option<String>>,
}

impl Default for FakeReleaseTarget {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(FakeReleaseState {
                declared: Version::new(0, 1, 0),
                published: Vec::new(),
                artifact_path: "dist/fake.pkg".to_string(),
                tag_scheme: None,
                outcomes: vec![PublishOutcome::Published],
                publish_index: 0,
                fail_apply: None,
                fail_package: None,
                fail_publish: None,
                fail_version_read: None,
                sbom_artifact: Some("sbom/fake.cdx.json".to_string()),
                fail_sbom: None,
                calls: Vec::new(),
                publish_token_envs: Vec::new(),
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

    /// Set the release tag scheme returned by `tag_scheme`.
    #[must_use]
    pub fn with_tag_scheme(self, scheme: TagScheme) -> Self {
        self.state().tag_scheme = Some(scheme);
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

    /// Make `publish` fail with a typed internal error before returning
    /// outcomes.
    #[must_use]
    pub fn with_publish_failure(self, message: impl Into<String>) -> Self {
        self.state().fail_publish = Some(message.into());
        self
    }

    /// Set the SBOM artifact path `sbom` returns (relative to the output dir).
    #[must_use]
    pub fn with_sbom_artifact(self, path: impl Into<String>) -> Self {
        self.state().sbom_artifact = Some(path.into());
        self
    }

    /// Make `sbom` report the ecosystem as having no SBOM tooling (`Ok(None)`).
    #[must_use]
    pub fn with_sbom_unsupported(self) -> Self {
        self.state().sbom_artifact = None;
        self
    }

    /// Make `sbom` fail with a typed internal error (a tool failure).
    #[must_use]
    pub fn with_sbom_failure(self, message: impl Into<String>) -> Self {
        self.state().fail_sbom = Some(message.into());
        self
    }

    /// Make the version-read calls (`declared_version` and `published_versions`)
    /// fail with a typed internal error — e.g. to model a target that cannot
    /// resolve a version for a module that has never been released.
    #[must_use]
    pub fn with_version_read_failure(self, message: impl Into<String>) -> Self {
        self.state().fail_version_read = Some(message.into());
        self
    }

    /// Snapshot the recorded release calls in call order.
    #[must_use]
    pub fn calls(&self) -> Vec<ReleaseCall> {
        self.state().calls.clone()
    }

    /// Snapshot the `token_env` name each `publish` call received (via the
    /// [`ReleaseCredentials`] the engine threaded from the resolved release
    /// settings), in call order. `None` entries are ambient-credential publishes.
    #[must_use]
    pub fn publish_token_envs(&self) -> Vec<Option<String>> {
        self.state().publish_token_envs.clone()
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
        let state = self.state();
        if let Some(message) = &state.fail_version_read {
            return Err(fake_error(message));
        }
        Ok(state.declared.clone())
    }

    fn published_versions(&self, module: &Module) -> AppResult<Vec<Version>> {
        self.record(ReleaseCall::PublishedVersions(module.id.clone()));
        let state = self.state();
        if let Some(message) = &state.fail_version_read {
            return Err(fake_error(message));
        }
        Ok(state.published.clone())
    }

    fn tag_scheme(&self, module: &Module, tag_format: Option<&str>) -> AppResult<TagScheme> {
        self.record(ReleaseCall::TagScheme {
            module: module.id.clone(),
            tag_format: tag_format.map(str::to_string),
        });
        let cached = self.state().tag_scheme.clone();
        if let Some(scheme) = cached {
            return Ok(scheme);
        }
        if let Some(format) = tag_format {
            return render_tag_format(format, module);
        }
        Ok(TagScheme::new(
            format!("{}/{}@", module.id.ecosystem.as_str(), module.id.name),
            "",
        ))
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

    fn publish(
        &self,
        module: &Module,
        _artifact: &Artifact,
        credentials: &ReleaseCredentials,
    ) -> AppResult<PublishOutcome> {
        self.record(ReleaseCall::Publish(module.id.clone()));
        let mut state = self.state();
        state
            .publish_token_envs
            .push(credentials.registry_token_env().map(str::to_string));
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

    fn sbom(&self, module: &Module, out_dir: &Path) -> AppResult<Option<Artifact>> {
        self.record(ReleaseCall::Sbom {
            module: module.id.clone(),
            out_dir: out_dir.display().to_string(),
        });
        let state = self.state();
        if let Some(message) = &state.fail_sbom {
            return Err(fake_error(message));
        }
        let sbom_artifact = state.sbom_artifact.clone();
        drop(state);
        let Some(path) = sbom_artifact else {
            return Ok(None);
        };
        // Write a small deterministic CycloneDX document so callers that stage
        // the artifact to a declared asset path have a real file to copy.
        let artifact = out_dir.join(path);
        std::fs::write(&artifact, b"{\"bomFormat\":\"CycloneDX\"}\n").map_err(|error| {
            AppError::new(
                ErrorCode::Internal,
                format!("fake sbom cannot write '{}': {error}", artifact.display()),
            )
        })?;
        Ok(Some(Artifact::new(artifact)))
    }
}

fn fake_error(message: &str) -> AppError {
    AppError::new(ErrorCode::Internal, message.to_string())
}

/// Render a `tag_format` template into a prefix/suffix [`TagScheme`] for the
/// double, substituting the module-scoped placeholders and splitting on
/// `{version}`.
#[allow(clippy::literal_string_with_formatting_args)]
fn render_tag_format(format: &str, module: &Module) -> AppResult<TagScheme> {
    let rendered = format
        .replace("{ecosystem}", module.id.ecosystem.as_str())
        .replace("{module}", &module.id.name)
        .replace("{channel}", "");
    let Some((prefix, suffix)) = rendered.split_once("{version}") else {
        return Err(AppError::invalid_input(
            "release.tag_format",
            "release tag template must contain {version}",
        ));
    };
    Ok(TagScheme::new(prefix, suffix))
}

#[cfg(test)]
mod tests {
    use rskit_version::semver::Version;
    use toven_model::{EcosystemId, Module, ModuleRef, RepoPath};
    use toven_ports::{Artifact, PublishOutcome, ReleaseCredentials, ReleaseTarget};

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
                .publish(
                    &module,
                    &Artifact::new("dist/x"),
                    &ReleaseCredentials::default(),
                )
                .expect("ok"),
            PublishOutcome::AlreadyPublished
        );
    }
}
