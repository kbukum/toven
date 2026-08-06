//! Shared [`ImagePhase`] double: [`FakeImagePhase`].
//!
//! Release-engine tests script image-publish outcomes and record calls here
//! instead of shelling to a real `docker buildx`/`cosign`. It is `Clone`
//! (shared state via `Arc<Mutex<…>>`) so a test can hold a recording handle
//! while the engine drives a boxed copy. It can be scripted to already hold a
//! tag at a digest (so the immutable preview and the divergent-tag fail-closed
//! path are exercised offline) or to fail outright.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_ports::{ImageOutcome, ImagePhase, ImagePublishOutcome, ImageRequest};

/// One image-publish call recorded by [`FakeImagePhase`].
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ImageCall {
    /// Working directory the image was built in.
    pub root: PathBuf,
    /// The image request the engine resolved.
    pub request: ImageRequest,
}

#[derive(Debug, Clone)]
struct FakeImageState {
    digest: String,
    outcome: ImageOutcome,
    /// A digest an existing tag already points at (scripts the preview and the
    /// divergent-tag fail-closed path).
    existing_digest: Option<String>,
    reference_digests: BTreeMap<String, String>,
    fail: Option<String>,
    calls: Vec<ImageCall>,
}

/// An [`ImagePhase`] with a scripted digest/outcome and call recording.
#[derive(Debug, Clone)]
pub struct FakeImagePhase {
    inner: Arc<Mutex<FakeImageState>>,
}

impl Default for FakeImagePhase {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(FakeImageState {
                digest: "sha256:feedface".to_string(),
                outcome: ImageOutcome::Pushed,
                existing_digest: None,
                reference_digests: BTreeMap::new(),
                fail: None,
                calls: Vec::new(),
            })),
        }
    }
}

impl FakeImagePhase {
    /// A phase that pushes a fresh `sha256:feedface` digest.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A phase that always fails with `message` (builder/cosign missing), so
    /// the release aborts before it claims to have pushed an image.
    #[must_use]
    pub fn failing(message: impl Into<String>) -> Self {
        let phase = Self::new();
        phase.state().fail = Some(message.into());
        phase
    }

    /// Script the digest the built image resolves to.
    #[must_use]
    pub fn with_digest(self, digest: impl Into<String>) -> Self {
        self.state().digest = digest.into();
        self
    }

    /// Script the publish outcome (e.g. an idempotent re-run that already held
    /// the digest).
    #[must_use]
    pub fn with_outcome(self, outcome: ImageOutcome) -> Self {
        self.state().outcome = outcome;
        self
    }

    /// Script the digest an existing tag already points at, as reported by
    /// `resolve_digest`. When it differs from the built digest, `publish_image`
    /// fails closed on the divergent tag.
    #[must_use]
    pub fn with_existing_digest(self, digest: impl Into<String>) -> Self {
        self.state().existing_digest = Some(digest.into());
        self
    }

    /// Script the digest a specific registry reference points at. References
    /// without an explicit entry fall back to [`Self::with_existing_digest`].
    #[must_use]
    pub fn with_reference_digest(
        self,
        reference: impl Into<String>,
        digest: impl Into<String>,
    ) -> Self {
        self.state()
            .reference_digests
            .insert(reference.into(), digest.into());
        self
    }

    /// Snapshot the recorded image-publish calls in call order.
    #[must_use]
    pub fn calls(&self) -> Vec<ImageCall> {
        self.state().calls.clone()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, FakeImageState> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl ImagePhase for FakeImagePhase {
    fn publish_image(&self, root: &Path, request: &ImageRequest) -> AppResult<ImagePublishOutcome> {
        let mut state = self.state();
        state.calls.push(ImageCall {
            root: root.to_path_buf(),
            request: request.clone(),
        });
        if let Some(message) = &state.fail {
            return Err(AppError::new(ErrorCode::Internal, message.clone()));
        }
        if let Some(existing) = &state.existing_digest
            && existing != &state.digest
        {
            return Err(AppError::invalid_input(
                "release.image",
                format!(
                    "tag already exists at digest {existing}, not the built {}; releases are \
                     immutable — cut a forward-fix version rather than move the tag",
                    state.digest
                ),
            ));
        }
        Ok(ImagePublishOutcome::new(
            state.outcome,
            state.digest.clone(),
            request.registries.clone(),
            request.sign,
        ))
    }

    fn resolve_digest(&self, _root: &Path, reference: &str) -> AppResult<Option<String>> {
        let state = self.state();
        Ok(state
            .reference_digests
            .get(reference)
            .cloned()
            .or_else(|| state.existing_digest.clone()))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use toven_ports::{ImageOutcome, ImagePhase, ImageRequest};

    use super::FakeImagePhase;

    fn request() -> ImageRequest {
        ImageRequest::new("services/api", "toven", "1.0.0")
            .with_registries(vec!["ghcr.io/acme".into()])
    }

    #[test]
    fn records_calls_and_returns_scripted_digest() {
        let phase = FakeImagePhase::new().with_digest("sha256:abc");
        let outcome = phase
            .publish_image(Path::new("/repo"), &request())
            .expect("ok");
        assert_eq!(outcome.digest, "sha256:abc");
        assert_eq!(outcome.outcome, ImageOutcome::Pushed);
        assert_eq!(phase.calls().len(), 1);
    }

    #[test]
    fn divergent_existing_tag_fails_closed() {
        let phase = FakeImagePhase::new()
            .with_digest("sha256:new")
            .with_existing_digest("sha256:old");
        let error = phase
            .publish_image(Path::new("/repo"), &request())
            .expect_err("divergent tag fails");
        assert!(error.to_string().contains("immutable"), "{error}");
        assert_eq!(phase.calls().len(), 1);
    }

    #[test]
    fn per_reference_digest_overrides_default_digest() {
        let phase = FakeImagePhase::new()
            .with_existing_digest("sha256:default")
            .with_reference_digest("ghcr.io/acme/toven:1.0.0", "sha256:primary");

        assert_eq!(
            phase
                .resolve_digest(Path::new("/repo"), "ghcr.io/acme/toven:1.0.0")
                .expect("resolves"),
            Some("sha256:primary".to_string())
        );
        assert_eq!(
            phase
                .resolve_digest(Path::new("/repo"), "docker.io/acme/toven:1.0.0")
                .expect("resolves"),
            Some("sha256:default".to_string())
        );
    }

    #[test]
    fn scripted_failure_surfaces_and_still_records() {
        let phase = FakeImagePhase::failing("buildx boom");
        let error = phase
            .publish_image(Path::new("/repo"), &request())
            .expect_err("fails");
        assert!(error.to_string().contains("buildx boom"));
        assert_eq!(phase.calls().len(), 1);
    }
}
