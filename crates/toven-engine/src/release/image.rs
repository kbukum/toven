//! `release image` verb and the buildx/cosign [`ImagePhase`] adapter.
//!
//! The engine owns image *policy*: which modules run the image phase (those
//! declaring `[…release.image]`), the resolved image name/tag rendered from the
//! module's declared version, the primary-plus-mirror registry set, whether the
//! pushed digest is signed, and the mutation-free `--dry-run` preview. The only
//! reusable primitive is "run a subprocess" ([`rskit_process`]);
//! [`BuildxImagePhase`] shells to `docker buildx` and `cosign` argv-only,
//! inheriting the ambient registry credentials — it embeds no secret and
//! captures none.
//!
//! Image publication is immutable: pushing a tag that already exists at a
//! *different* digest fails closed, and recovery is a forward-fix version, never
//! a moved tag.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rskit_errors::{AppError, AppResult};
use rskit_fs::TempFile;
use rskit_fs::sync_io::file::read_string_bounded;
use rskit_process::{CapturedIo, OutputPolicy, ProcessConfig, ProcessIo, ProcessSpec, run};
use rskit_util::Template;
use toven_ports::{ImageOutcome, ImagePhase, ImageRequest, Provider, ReleaseVar, Reporter};

use super::plan::{release_targets, resolve_release_settings};
use crate::config::Document;
use crate::federation::resolve::PathDriverLocator;
use crate::plan::{PlanRequest, prepare_front};

/// Hard bound on captured tool output (256 KiB) — a build log is terse in the
/// captured tail; this only guards against a pathological stream.
const MAX_IMAGE_OUTPUT_BYTES: usize = 256 * 1024;

/// Hard bound on the `docker buildx` metadata file read (64 KiB) — it holds a
/// small JSON object of build outputs, so this only guards a pathological file.
const MAX_IMAGE_METADATA_BYTES: u64 = 64 * 1024;

/// Timeout for a single image build/push/sign invocation. Builds and registry
/// round-trips are slower than a local command.
const IMAGE_TIMEOUT: Duration = Duration::from_mins(30);

/// The resolved status of one module's image phase.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub enum ImagePhaseStatus {
    /// The built digest was pushed to at least one registry.
    Pushed,
    /// Every registry already held the built digest (idempotent re-run).
    AlreadyComplete,
    /// `--dry-run`: no tag exists yet, so a push would create it.
    WouldPush,
    /// `--dry-run`: a tag already exists (its digest is reported).
    AlreadyPresent,
}

impl ImagePhaseStatus {
    /// Canonical wire/report name for the status.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pushed => "pushed",
            Self::AlreadyComplete => "already-complete",
            Self::WouldPush => "would-push",
            Self::AlreadyPresent => "already-present",
        }
    }
}

/// A read-only projection of one module's image outcome.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ImageModuleOutcome {
    /// The module label the image was built for.
    pub module: String,
    /// The fully-qualified references the image is published to (primary then
    /// mirrors).
    pub references: Vec<String>,
    /// The pushed or currently-present digest (`sha256:...`); `None` in a
    /// preview when the tag does not yet exist.
    pub digest: Option<String>,
    /// Whether the pushed digest was (or would be) signed.
    pub signed: bool,
    /// The resolved status of the phase for this module.
    pub status: ImagePhaseStatus,
}

/// A read-only projection of the image phase over the release scope.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ImageReport {
    /// Whether this report is a mutation-free `--dry-run` preview.
    pub preview: bool,
    /// Per-module image outcomes, in module-label order.
    pub images: Vec<ImageModuleOutcome>,
}

/// Options controlling the image phase.
#[derive(Debug, Clone, Copy, Default)]
pub struct ImageOptions {
    /// Preview the phase mutation-free: resolve existing digests but never
    /// build, push, or sign.
    pub dry_run: bool,
}

/// Build, push, and sign the configured container image per releasable module
/// that declares `[…release.image]`.
///
/// Only modules with an image block participate. With `options.dry_run`, the
/// phase is a mutation-free preview: it resolves each reference's existing
/// digest but never builds or pushes. Otherwise it builds each image once,
/// pushes it to the primary registry plus mirrors immutably, and signs the
/// pushed digest when configured.
///
/// # Errors
/// Fails closed with a typed error when no module declares an image block, and
/// propagates configuration/discovery/graph failures, template-render failures,
/// and build/push/sign failures — including the immutable divergent-tag
/// conflict.
pub fn release_image(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    image_phase: &dyn ImagePhase,
    options: ImageOptions,
    reporter: &mut dyn Reporter,
) -> AppResult<ImageReport> {
    let locator = PathDriverLocator::new();
    let context = prepare_front(
        &request.project_root,
        document,
        providers,
        &locator,
        reporter,
    )?;
    let targets = release_targets(&context)?;
    let settings = resolve_release_settings(&context, &targets)?;
    let project_root = request.project_root.as_path();

    let mut images = Vec::new();
    for (module_key, image_request) in resolved_image_requests(&context, &targets, &settings)? {
        let outcome = if options.dry_run {
            preview_image(project_root, image_phase, &module_key, &image_request)?
        } else {
            publish_one(project_root, image_phase, &module_key, &image_request)?
        };
        images.push(outcome);
    }

    if images.is_empty() {
        return Err(AppError::invalid_input(
            "release.image",
            "no module declares an image phase; add a […release.image] block to the module that \
             ships a container image",
        ));
    }
    images.sort_by(|left, right| left.module.cmp(&right.module));

    Ok(ImageReport {
        preview: options.dry_run,
        images,
    })
}

/// Resolve the [`ImageRequest`] for every module that declares a
/// `[…release.image]` block, paired with the module's label. Each request's
/// name/tag are rendered from the module's declared version, so both the
/// `image` phase and the `provenance` phase (which attests the pushed image
/// digests) resolve identical references. Modules without an image block are
/// skipped.
///
/// # Errors
/// Propagates a name/tag template-render failure or a declared-version lookup
/// failure.
pub(super) fn resolved_image_requests(
    context: &crate::plan::PlanContext,
    targets: &super::ReleaseTargets,
    settings: &std::collections::BTreeMap<toven_model::ModuleKey, super::ResolvedReleaseSettings>,
) -> AppResult<Vec<(String, ImageRequest)>> {
    let mut requests = Vec::new();
    for module in &context.federation.modules {
        let key = (module.member.clone(), module.id.ecosystem.clone());
        let Some(target) = targets.get(&key) else {
            continue;
        };
        let Some(resolved) = settings.get(&module.key()) else {
            continue;
        };
        let Some(image_config) = &resolved.image else {
            continue;
        };

        let version = target.declared_version(module)?;
        let name = render_template(&image_config.name, "release.image.name", module, &version)?;
        let tag = render_template(
            image_config.tag_template(),
            "release.image.tag",
            module,
            &version,
        )?;
        let mut image_request = ImageRequest::new(build_context(image_config), name, tag)
            .with_registries(registries(image_config))
            .with_sign(image_config.sign);
        if let Some(dockerfile) = &image_config.dockerfile {
            image_request = image_request.with_dockerfile(PathBuf::from(dockerfile));
        }
        requests.push((module.key().to_string(), image_request));
    }
    Ok(requests)
}

/// Preview one module's image references without building or pushing.
fn preview_image(
    project_root: &Path,
    image_phase: &dyn ImagePhase,
    module_key: &str,
    request: &ImageRequest,
) -> AppResult<ImageModuleOutcome> {
    let mut present: Option<String> = None;
    for reference in request.references() {
        if let Some(digest) = image_phase.resolve_digest(project_root, &reference)? {
            present = Some(digest);
        }
    }
    let (status, digest) = present.map_or((ImagePhaseStatus::WouldPush, None), |digest| {
        (ImagePhaseStatus::AlreadyPresent, Some(digest))
    });
    Ok(ImageModuleOutcome {
        module: module_key.to_string(),
        references: request.references(),
        digest,
        signed: request.sign,
        status,
    })
}

/// Build, push, and sign one module's image immutably.
fn publish_one(
    project_root: &Path,
    image_phase: &dyn ImagePhase,
    module_key: &str,
    request: &ImageRequest,
) -> AppResult<ImageModuleOutcome> {
    let published = image_phase.publish_image(project_root, request)?;
    let status = match published.outcome {
        ImageOutcome::AlreadyComplete => ImagePhaseStatus::AlreadyComplete,
        _ => ImagePhaseStatus::Pushed,
    };
    Ok(ImageModuleOutcome {
        module: module_key.to_string(),
        references: request.references(),
        digest: Some(published.digest),
        signed: published.signed,
        status,
    })
}

/// The build context path for an image config, defaulting to the project root
/// (`.`).
fn build_context(config: &toven_ports::ImageConfig) -> PathBuf {
    config
        .context
        .as_deref()
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
}

/// The primary-then-mirror registry list for an image config.
fn registries(config: &toven_ports::ImageConfig) -> Vec<String> {
    let mut registries = vec![config.registry.clone()];
    registries.extend(config.mirrors.iter().cloned());
    registries
}

/// Render a name/tag template against the [`ReleaseVar`] vocabulary, using the
/// module's declared version and an empty (stable) channel.
fn render_template(
    template: &str,
    field: &str,
    module: &toven_model::Module,
    version: &rskit_version::semver::Version,
) -> AppResult<String> {
    let parsed = Template::parse(template, ReleaseVar::ALL).map_err(|error| {
        AppError::invalid_input(field, format!("invalid image template: {error}")).with_cause(error)
    })?;
    parsed
        .render_with(|placeholder| match placeholder {
            ReleaseVar::Version => Ok(version.to_string()),
            ReleaseVar::Ecosystem => Ok(module.id.ecosystem.to_string()),
            ReleaseVar::Module => Ok(module.id.name.clone()),
            ReleaseVar::Channel => Ok(String::new()),
            _ => Err(AppError::new(
                rskit_errors::ErrorCode::Internal,
                "unknown image template placeholder",
            )),
        })
        .map_err(|error| {
            AppError::invalid_input(field, format!("failed to render image template: {error}"))
                .with_cause(error)
        })
}

/// A buildx/cosign-backed [`ImagePhase`].
///
/// Construction is stateless. `docker buildx` and `cosign` are invoked
/// argv-only through [`rskit_process`]; the ambient registry/OIDC credentials
/// the runner provides are inherited, and no secret is placed on argv or
/// captured.
#[derive(Debug, Clone)]
pub struct BuildxImagePhase {
    timeout: Duration,
}

impl BuildxImagePhase {
    /// Construct a buildx image phase with the default per-invocation timeout.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            timeout: IMAGE_TIMEOUT,
        }
    }
}

impl Default for BuildxImagePhase {
    fn default() -> Self {
        Self::new()
    }
}

impl ImagePhase for BuildxImagePhase {
    fn publish_image(
        &self,
        root: &Path,
        request: &ImageRequest,
    ) -> AppResult<toven_ports::ImagePublishOutcome> {
        let references = request.references();
        if references.is_empty() {
            return Err(AppError::invalid_input(
                "release.image.registry",
                "no registry configured for the image push",
            ));
        }

        // Build once (without pushing) to learn the digest the push would
        // publish. Knowing the built digest before any mutation is what lets the
        // immutable guard distinguish an idempotent re-run from a divergent tag.
        let built = self.build_digest(root, request, &references)?;

        // Immutable guard: compare each existing tag to the built digest. A tag
        // already at the built digest is an idempotent re-run; a tag at a
        // *different* digest fails closed — recovery is a forward-fix version,
        // never a moved tag.
        let mut all_present = true;
        for reference in &references {
            match self.resolve_digest(root, reference)? {
                Some(existing) if existing == built => {}
                Some(existing) => {
                    return Err(AppError::invalid_input(
                        "release.image",
                        format!(
                            "image tag '{reference}' already exists at digest {existing}, not the \
                             built {built}; releases are immutable — cut a forward-fix version \
                             rather than move the tag"
                        ),
                    ));
                }
                None => all_present = false,
            }
        }
        if all_present {
            return Ok(toven_ports::ImagePublishOutcome::new(
                ImageOutcome::AlreadyComplete,
                built,
                request.registries.clone(),
                request.sign,
            ));
        }

        // At least one reference is missing: push the built image to every
        // reference (a no-op re-push for any already at the built digest) and
        // sign the pushed digest.
        self.run(root, "docker", buildx_push_argv(request, &references)?)?;
        if request.sign {
            for reference in &references {
                self.run(root, "cosign", cosign_argv(&format!("{reference}@{built}")))?;
            }
        }
        Ok(toven_ports::ImagePublishOutcome::new(
            ImageOutcome::Pushed,
            built,
            request.registries.clone(),
            request.sign,
        ))
    }

    fn resolve_digest(&self, root: &Path, reference: &str) -> AppResult<Option<String>> {
        let spec = ProcessSpec::new("docker")
            .args(inspect_argv(reference))
            .dir(root);
        let config = ProcessConfig::default()
            .with_timeout(Some(self.timeout))
            .with_io(ProcessIo::captured(CapturedIo::new().with_output(
                OutputPolicy::captured().with_max_output_bytes(MAX_IMAGE_OUTPUT_BYTES),
            )));
        let result = run(&spec, &config)?;
        if result.success() {
            let digest = result.stdout.trim().to_string();
            if digest.is_empty() {
                return Ok(None);
            }
            return Ok(Some(digest));
        }
        // A missing tag is the expected "not found" signal, not a failure.
        Ok(None)
    }
}

impl BuildxImagePhase {
    /// Run an argv-only `docker`/`cosign` invocation rooted at `root`, failing
    /// closed on a spawn or non-zero exit.
    fn run(&self, root: &Path, program: &str, argv: Vec<String>) -> AppResult<()> {
        let spec = ProcessSpec::new(program).args(argv).dir(root);
        let config = ProcessConfig::default()
            .with_timeout(Some(self.timeout))
            .with_io(ProcessIo::captured(CapturedIo::new().with_output(
                OutputPolicy::captured().with_max_output_bytes(MAX_IMAGE_OUTPUT_BYTES),
            )));
        run(&spec, &config)?.check()?;
        Ok(())
    }

    /// Build the image once (without pushing) and return the digest the push
    /// would publish, read from the `docker buildx` metadata file. Failing
    /// closed when the metadata carries no digest keeps a build that produced
    /// nothing from being reported as a successful publish.
    fn build_digest(
        &self,
        root: &Path,
        request: &ImageRequest,
        references: &[String],
    ) -> AppResult<String> {
        let metadata = TempFile::with_extension("json")?;
        self.run(
            root,
            "docker",
            buildx_build_argv(request, references, metadata.path())?,
        )?;
        let text = read_string_bounded(metadata.path(), MAX_IMAGE_METADATA_BYTES)?;
        parse_metadata_digest(&text)
    }
}

/// Extract the `containerimage.digest` a `docker buildx build --metadata-file`
/// run recorded (`sha256:...`), failing closed when it is absent.
fn parse_metadata_digest(text: &str) -> AppResult<String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|error| {
        AppError::invalid_input(
            "release.image.digest",
            format!("could not parse buildx metadata: {error}"),
        )
        .with_cause(error)
    })?;
    value
        .get("containerimage.digest")
        .and_then(serde_json::Value::as_str)
        .filter(|digest| !digest.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            AppError::invalid_input(
                "release.image.digest",
                "buildx metadata has no 'containerimage.digest'; the build produced no image",
            )
        })
}

/// Build the argv-only `docker buildx build --load` invocation that builds the
/// image into the local store and records its digest to `metadata`, without
/// pushing. The subsequent [`buildx_push_argv`] run reuses the build cache.
fn buildx_build_argv(
    request: &ImageRequest,
    references: &[String],
    metadata: &Path,
) -> AppResult<Vec<String>> {
    let mut argv = vec![
        "buildx".to_string(),
        "build".to_string(),
        "--metadata-file".to_string(),
        path_arg(metadata)?,
        "--load".to_string(),
    ];
    append_build_targets(&mut argv, request, references)?;
    Ok(argv)
}

/// Build the argv-only `docker buildx build --push` invocation.
fn buildx_push_argv(request: &ImageRequest, references: &[String]) -> AppResult<Vec<String>> {
    let mut argv = vec![
        "buildx".to_string(),
        "build".to_string(),
        "--push".to_string(),
    ];
    append_build_targets(&mut argv, request, references)?;
    Ok(argv)
}

/// Append the shared `--file`/`--tag`/context tail every `buildx build`
/// invocation carries.
fn append_build_targets(
    argv: &mut Vec<String>,
    request: &ImageRequest,
    references: &[String],
) -> AppResult<()> {
    if let Some(dockerfile) = &request.dockerfile {
        argv.push("--file".to_string());
        argv.push(path_arg(dockerfile)?);
    }
    for reference in references {
        argv.push("--tag".to_string());
        argv.push(reference.clone());
    }
    argv.push(path_arg(&request.context)?);
    Ok(())
}

/// Build the argv-only `docker buildx imagetools inspect` invocation that reads
/// a reference's digest.
fn inspect_argv(reference: &str) -> Vec<String> {
    vec![
        "buildx".to_string(),
        "imagetools".to_string(),
        "inspect".to_string(),
        "--format".to_string(),
        "{{.Manifest.Digest}}".to_string(),
        reference.to_string(),
    ]
}

/// Build the argv-only `cosign sign` invocation for a digest-pinned reference.
/// Keyless (no `--key`) by default; `--yes` skips the interactive confirmation
/// so the non-interactive CI signer never blocks.
fn cosign_argv(reference: &str) -> Vec<String> {
    vec![
        "sign".to_string(),
        "--yes".to_string(),
        reference.to_string(),
    ]
}

/// Render a path as a UTF-8 argv token, failing closed on a non-UTF-8 path.
fn path_arg(path: &Path) -> AppResult<String> {
    path.to_str().map(str::to_string).ok_or_else(|| {
        AppError::invalid_input(
            "release.image.path",
            format!("path '{}' is not valid UTF-8", path.display()),
        )
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use rskit_config::RawValue;
    use serde_json::json;
    use toven_model::{AbsPath, EcosystemId, Module, ModuleRef, RepoPath};
    use toven_ports::{
        CommonEcosystemConfig, DiscoverResponse, ImageConfig, ImageOutcome, Provider,
        ReleaseConfig, TaskIntent,
    };
    use toven_testkit::{
        FakeConfiguredAdapter, FakeImagePhase, FakeProvider, FakeReleaseTarget, RecordingReporter,
    };

    use super::{
        ImageOptions, ImagePhaseStatus, buildx_build_argv, buildx_push_argv, inspect_argv,
        parse_metadata_digest, release_image,
    };
    use crate::config::{Document, ProjectConfig, TovenConfig};
    use crate::plan::PlanRequest;
    use rskit_version::semver::Version;
    use toven_ports::ImageRequest;

    fn eid(id: &str) -> EcosystemId {
        EcosystemId::new(id).unwrap()
    }

    fn module(name: &str) -> Module {
        Module::new(
            ModuleRef::new(eid("rust"), name).unwrap(),
            RepoPath::new(format!("crates/{name}")).unwrap(),
        )
    }

    fn document() -> Document {
        let mut ecosystems = BTreeMap::new();
        ecosystems.insert(eid("rust"), RawValue::from(json!({ "release": {} })));
        Document {
            project: ProjectConfig {
                name: "demo".to_string(),
                root: ".".to_string(),
                base_ref: None,
            },
            toven: TovenConfig::default(),
            groups: BTreeMap::new(),
            overlays: Vec::new(),
            ecosystems,
            modules: BTreeMap::new(),
            members: Vec::new(),
        }
    }

    fn request() -> PlanRequest {
        PlanRequest::new(
            "r1",
            "demo",
            TaskIntent::resolve("release"),
            AbsPath::new("/repo").unwrap(),
        )
    }

    /// A provider whose module declares an image block (or not) and reports the
    /// given declared version.
    fn provider_with_image(image: Option<ImageConfig>, version: Version) -> FakeProvider {
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![module("app")];
        let common = CommonEcosystemConfig {
            release: ReleaseConfig {
                image,
                ..ReleaseConfig::default()
            },
            ..CommonEcosystemConfig::default()
        };
        let adapter = FakeConfiguredAdapter::new(eid("rust"))
            .with_response(response)
            .with_release_target(FakeReleaseTarget::new().with_declared_version(version))
            .with_common(common);
        FakeProvider::new(eid("rust")).with_adapter(adapter)
    }

    fn image_config() -> ImageConfig {
        ImageConfig {
            registry: "ghcr.io/acme".into(),
            mirrors: vec!["docker.io/acme".into()],
            name: "toven".into(),
            tag: Some("{version}".into()),
            context: Some("services/api".into()),
            dockerfile: None,
            sign: true,
        }
    }

    #[test]
    fn image_builds_once_pushes_primary_and_mirrors_and_signs() {
        let provider = provider_with_image(Some(image_config()), Version::new(1, 2, 3));
        let providers: Vec<&dyn Provider> = vec![&provider];
        let phase = FakeImagePhase::new().with_digest("sha256:abc");
        let mut reporter = RecordingReporter::new();

        let report = release_image(
            &request(),
            &document(),
            &providers,
            &phase,
            ImageOptions::default(),
            &mut reporter,
        )
        .expect("image phase runs");

        assert!(!report.preview);
        assert_eq!(report.images.len(), 1);
        let outcome = &report.images[0];
        assert_eq!(outcome.status, ImagePhaseStatus::Pushed);
        assert_eq!(outcome.digest.as_deref(), Some("sha256:abc"));
        assert!(outcome.signed);
        assert_eq!(
            outcome.references,
            vec![
                "ghcr.io/acme/toven:1.2.3".to_string(),
                "docker.io/acme/toven:1.2.3".to_string(),
            ]
        );
        // Exactly one build/push call was recorded (build once, push everywhere).
        let calls = phase.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].request.registries,
            vec!["ghcr.io/acme", "docker.io/acme"]
        );
    }

    #[test]
    fn image_dry_run_previews_without_pushing() {
        let provider = provider_with_image(Some(image_config()), Version::new(1, 0, 0));
        let providers: Vec<&dyn Provider> = vec![&provider];
        let phase = FakeImagePhase::new();
        let mut reporter = RecordingReporter::new();

        let report = release_image(
            &request(),
            &document(),
            &providers,
            &phase,
            ImageOptions { dry_run: true },
            &mut reporter,
        )
        .expect("preview runs");

        assert!(report.preview);
        assert_eq!(report.images[0].status, ImagePhaseStatus::WouldPush);
        // A preview never builds or pushes.
        assert!(phase.calls().is_empty(), "preview must not push");
    }

    #[test]
    fn image_dry_run_reports_an_existing_tag() {
        let provider = provider_with_image(Some(image_config()), Version::new(1, 0, 0));
        let providers: Vec<&dyn Provider> = vec![&provider];
        let phase = FakeImagePhase::new().with_existing_digest("sha256:live");
        let mut reporter = RecordingReporter::new();

        let report = release_image(
            &request(),
            &document(),
            &providers,
            &phase,
            ImageOptions { dry_run: true },
            &mut reporter,
        )
        .expect("preview runs");

        assert_eq!(report.images[0].status, ImagePhaseStatus::AlreadyPresent);
        assert_eq!(report.images[0].digest.as_deref(), Some("sha256:live"));
        assert!(phase.calls().is_empty());
    }

    #[test]
    fn image_fails_closed_on_a_divergent_existing_tag() {
        let provider = provider_with_image(Some(image_config()), Version::new(2, 0, 0));
        let providers: Vec<&dyn Provider> = vec![&provider];
        let phase = FakeImagePhase::new()
            .with_digest("sha256:new")
            .with_existing_digest("sha256:old");
        let mut reporter = RecordingReporter::new();

        let error = release_image(
            &request(),
            &document(),
            &providers,
            &phase,
            ImageOptions::default(),
            &mut reporter,
        )
        .expect_err("a divergent tag must fail closed");
        assert!(error.to_string().contains("immutable"), "{error}");
    }

    #[test]
    fn image_without_a_block_is_a_typed_error() {
        let provider = provider_with_image(None, Version::new(1, 0, 0));
        let providers: Vec<&dyn Provider> = vec![&provider];
        let phase = FakeImagePhase::new();
        let mut reporter = RecordingReporter::new();

        let error = release_image(
            &request(),
            &document(),
            &providers,
            &phase,
            ImageOptions::default(),
            &mut reporter,
        )
        .expect_err("no image block must be a typed error");
        assert!(
            error
                .to_string()
                .contains("no module declares an image phase"),
            "{error}"
        );
    }

    #[test]
    fn buildx_push_argv_pushes_every_reference_and_targets_the_context() {
        let request = ImageRequest::new("services/api", "toven", "1.2.3")
            .with_registries(vec!["ghcr.io/acme".into(), "docker.io/acme".into()]);
        let argv = buildx_push_argv(&request, &request.references()).expect("argv");
        assert_eq!(argv[0], "buildx");
        assert!(argv.iter().any(|token| token == "--push"));
        assert!(argv.iter().any(|token| token == "ghcr.io/acme/toven:1.2.3"));
        assert!(
            argv.iter()
                .any(|token| token == "docker.io/acme/toven:1.2.3")
        );
        assert_eq!(argv.last().map(String::as_str), Some("services/api"));
    }

    #[test]
    fn buildx_build_argv_captures_the_digest_without_pushing() {
        let request = ImageRequest::new("services/api", "toven", "1.2.3")
            .with_registries(vec!["ghcr.io/acme".into()]);
        let argv = buildx_build_argv(&request, &request.references(), Path::new("/tmp/meta.json"))
            .expect("argv");
        assert!(argv.iter().any(|token| token == "--metadata-file"));
        assert!(argv.iter().any(|token| token == "/tmp/meta.json"));
        assert!(argv.iter().any(|token| token == "--load"));
        assert!(
            !argv.iter().any(|token| token == "--push"),
            "the digest-capture build must not push"
        );
        assert!(argv.iter().any(|token| token == "ghcr.io/acme/toven:1.2.3"));
        assert_eq!(argv.last().map(String::as_str), Some("services/api"));
    }

    #[test]
    fn parse_metadata_digest_reads_containerimage_digest() {
        let digest = parse_metadata_digest(r#"{"containerimage.digest":"sha256:abc"}"#)
            .expect("digest present");
        assert_eq!(digest, "sha256:abc");
    }

    #[test]
    fn parse_metadata_digest_fails_closed_without_a_digest() {
        assert!(parse_metadata_digest(r#"{"other":"value"}"#).is_err());
        assert!(parse_metadata_digest("not json").is_err());
    }

    #[test]
    fn inspect_argv_reads_the_manifest_digest() {
        let argv = inspect_argv("ghcr.io/acme/toven:1.2.3");
        assert!(argv.iter().any(|token| token == "inspect"));
        assert!(argv.iter().any(|token| token == "ghcr.io/acme/toven:1.2.3"));
    }

    #[test]
    fn image_outcome_maps_already_complete() {
        let provider = provider_with_image(Some(image_config()), Version::new(1, 0, 0));
        let providers: Vec<&dyn Provider> = vec![&provider];
        let phase = FakeImagePhase::new().with_outcome(ImageOutcome::AlreadyComplete);
        let mut reporter = RecordingReporter::new();

        let report = release_image(
            &request(),
            &document(),
            &providers,
            &phase,
            ImageOptions::default(),
            &mut reporter,
        )
        .expect("runs");
        assert_eq!(report.images[0].status, ImagePhaseStatus::AlreadyComplete);
    }

    #[test]
    fn image_surfaces_a_publish_failure() {
        let provider = provider_with_image(Some(image_config()), Version::new(1, 0, 0));
        let providers: Vec<&dyn Provider> = vec![&provider];
        let phase = FakeImagePhase::failing("buildx missing");
        let mut reporter = RecordingReporter::new();

        let error = release_image(
            &request(),
            &document(),
            &providers,
            &phase,
            ImageOptions::default(),
            &mut reporter,
        )
        .expect_err("a publish failure must surface");
        assert!(error.to_string().contains("buildx missing"), "{error}");
    }
}
