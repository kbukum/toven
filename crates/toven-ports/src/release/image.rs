//! The container-image port — build a tagged image once, push it to a primary
//! registry plus mirrors immutably, and sign the pushed digest.
//!
//! The engine owns image *policy*: which context/Dockerfile builds the image,
//! the resolved image name/tag, the primary-plus-mirror registry set, whether
//! the pushed digest is signed, and the mutation-free preview. This port is the
//! thin sliver: build once, push everywhere, sign, and report the digest — or,
//! read-only, resolve the digest a registry reference currently points at.
//!
//! Image publication is immutable: pushing a tag that already exists with a
//! *different* digest fails closed, and recovery is a forward-fix version, never
//! a moved tag. Implementations invoke their build/sign tooling argv-only (never
//! a shell string) and read any registry credential from the ambient
//! environment only — never logging it or placing it on argv.

use std::path::{Path, PathBuf};

use rskit_errors::AppResult;

/// A fully-resolved container-image build-and-push request for one module.
///
/// The engine resolves every field before the adapter runs: `context` and
/// `dockerfile` locate the build, `name`/`tag` are the already-rendered image
/// reference components, `registries` is the primary registry followed by its
/// mirrors (the first entry is authoritative), and `sign` selects whether the
/// pushed digest is cosign-signed. The adapter never re-derives any of these.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ImageRequest {
    /// Project-relative build context passed to the image builder.
    pub context: PathBuf,
    /// Optional Dockerfile path; `None` uses the builder's default
    /// (`<context>/Dockerfile`).
    pub dockerfile: Option<PathBuf>,
    /// Resolved image name (e.g. `toven`), without a registry or tag.
    pub name: String,
    /// Resolved image tag (e.g. `1.2.3`).
    pub tag: String,
    /// Target registries: the first is the primary, the rest are mirrors. The
    /// same image digest is pushed to every entry.
    pub registries: Vec<String>,
    /// Whether the pushed digest is signed (keyless cosign by default).
    pub sign: bool,
}

impl ImageRequest {
    /// Construct an image request for `name`:`tag` built from `context`, with no
    /// dockerfile override, no registries, and signing enabled.
    #[must_use]
    pub fn new(
        context: impl Into<PathBuf>,
        name: impl Into<String>,
        tag: impl Into<String>,
    ) -> Self {
        Self {
            context: context.into(),
            dockerfile: None,
            name: name.into(),
            tag: tag.into(),
            registries: Vec::new(),
            sign: true,
        }
    }

    /// Override the Dockerfile path used for the build.
    #[must_use]
    pub fn with_dockerfile(mut self, dockerfile: impl Into<PathBuf>) -> Self {
        self.dockerfile = Some(dockerfile.into());
        self
    }

    /// Set the primary-plus-mirror registry list (the first entry is the
    /// primary).
    #[must_use]
    pub fn with_registries(mut self, registries: Vec<String>) -> Self {
        self.registries = registries;
        self
    }

    /// Set whether the pushed digest is signed.
    #[must_use]
    pub const fn with_sign(mut self, sign: bool) -> Self {
        self.sign = sign;
        self
    }

    /// The primary registry (the first entry), if any.
    #[must_use]
    pub fn primary(&self) -> Option<&str> {
        self.registries.first().map(String::as_str)
    }

    /// The fully-qualified `registry/name:tag` reference for each registry, in
    /// primary-then-mirror order.
    #[must_use]
    pub fn references(&self) -> Vec<String> {
        self.registries
            .iter()
            .map(|registry| {
                format!(
                    "{}/{}:{}",
                    registry.trim_end_matches('/'),
                    self.name,
                    self.tag
                )
            })
            .collect()
    }
}

/// The outcome of pushing a tagged image to a registry set.
///
/// Image publication is immutable: a tag is either newly pushed or an identical
/// digest already existed and was verified. A tag that already exists with a
/// *different* digest is never overwritten — the adapter fails closed instead,
/// so this enum has no "updated" outcome.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub enum ImageOutcome {
    /// The tag did not exist (on at least one registry), so the built digest
    /// was pushed.
    Pushed,
    /// Every target registry already held the tag at the built digest (an
    /// idempotent re-run), so nothing was mutated.
    AlreadyComplete,
}

impl ImageOutcome {
    /// Canonical wire/report name for the outcome.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pushed => "pushed",
            Self::AlreadyComplete => "already-complete",
        }
    }
}

/// The result of publishing one module's image: the outcome, the pushed digest,
/// the registries it landed on, and whether it was signed.
#[derive(Debug, Clone, Eq, PartialEq)]
#[non_exhaustive]
pub struct ImagePublishOutcome {
    /// Whether the digest was freshly pushed or already complete.
    pub outcome: ImageOutcome,
    /// The pushed image digest (`sha256:...`).
    pub digest: String,
    /// The registries the digest was pushed to (primary then mirrors).
    pub registries: Vec<String>,
    /// Whether the pushed digest was signed.
    pub signed: bool,
}

impl ImagePublishOutcome {
    /// Construct a publish outcome.
    #[must_use]
    pub fn new(
        outcome: ImageOutcome,
        digest: impl Into<String>,
        registries: Vec<String>,
        signed: bool,
    ) -> Self {
        Self {
            outcome,
            digest: digest.into(),
            registries,
            signed,
        }
    }
}

/// The container-image port: build a tagged image once and push it to a primary
/// registry plus mirrors immutably, signing the pushed digest.
///
/// Object-safe so the engine can inject a `&dyn ImagePhase`. Implementations
/// build the image from `request.context`/`request.dockerfile`, push
/// `request.name`:`request.tag` to every entry in `request.registries`
/// (primary first), and — when `request.sign` — cosign-sign the resulting
/// digest. Pushing a tag that already exists with a **different** digest fails
/// closed with a typed conflict error; recovery is a forward-fix version, never
/// a moved tag.
pub trait ImagePhase: Send + Sync {
    /// Build the image once and push it to every registry in `request`,
    /// signing the pushed digest when `request.sign`.
    ///
    /// # Errors
    /// Fails closed on a build/push/sign spawn or non-zero exit, and returns a
    /// typed conflict error when a target tag already exists at a different
    /// digest than the freshly built one.
    fn publish_image(&self, root: &Path, request: &ImageRequest) -> AppResult<ImagePublishOutcome>;

    /// The digest a registry `reference` (`registry/name:tag`) currently points
    /// at, or `None` when the tag does not yet exist — a read-only registry
    /// query.
    ///
    /// The mutation-free preview uses it to report what a push would do without
    /// building or pushing, and the immutable publish path uses it to detect a
    /// divergent existing tag and fail closed. The working directory `root`
    /// locates the repository the image belongs to.
    ///
    /// # Errors
    /// Propagates a registry-tool spawn/IO failure or a non-zero exit that is
    /// not the "tag not found" signal.
    fn resolve_digest(&self, root: &Path, reference: &str) -> AppResult<Option<String>>;
}

#[cfg(test)]
mod tests {
    use super::{ImageOutcome, ImageRequest};

    #[test]
    fn request_builder_sets_fields_and_defaults_to_signed() {
        let request = ImageRequest::new("services/api", "toven", "1.2.3")
            .with_dockerfile("services/api/Dockerfile")
            .with_registries(vec!["ghcr.io/acme".into(), "docker.io/acme".into()]);
        assert_eq!(request.name, "toven");
        assert_eq!(request.tag, "1.2.3");
        assert!(request.sign, "signing is the keyless default");
        assert_eq!(request.primary(), Some("ghcr.io/acme"));
        assert_eq!(
            request.references(),
            vec![
                "ghcr.io/acme/toven:1.2.3".to_string(),
                "docker.io/acme/toven:1.2.3".to_string(),
            ]
        );
    }

    #[test]
    fn references_trim_a_trailing_slash_on_the_registry() {
        let request =
            ImageRequest::new(".", "toven", "1.0.0").with_registries(vec!["ghcr.io/acme/".into()]);
        assert_eq!(
            request.references(),
            vec!["ghcr.io/acme/toven:1.0.0".to_string()]
        );
    }

    #[test]
    fn outcome_names_are_stable() {
        assert_eq!(ImageOutcome::Pushed.as_str(), "pushed");
        assert_eq!(ImageOutcome::AlreadyComplete.as_str(), "already-complete");
    }
}
