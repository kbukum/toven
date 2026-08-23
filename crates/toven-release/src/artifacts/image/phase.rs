use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use rskit_errors::{AppError, AppResult};
use rskit_util::Template;
use tokio_util::sync::CancellationToken;
use toven_core::config::Document;
use toven_core::federation::resolve::PathDriverLocator;
use toven_core::plan::{PlanRequest, prepare_front};
use toven_ports::{ImageOutcome, ImagePhase, ImageRequest, Provider, ReleaseVar, Reporter};
use toven_runtime::{Completed, UnitOperation, UnitSpec};

use crate::planning::plan::{release_targets, resolve_release_settings};

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
    readers: &toven_core::federation::baseline::MemberVcsReaders<'_>,
    image_phase: &dyn ImagePhase,
    options: ImageOptions,
    reporter: &mut dyn Reporter,
) -> AppResult<ImageReport> {
    let inputs = ImageInputs::gather(request, document, providers, readers, options, reporter)?;
    let mut images = Vec::new();
    for unit in &inputs.units {
        images.push(image_for(&inputs, image_phase, unit)?);
    }
    Ok(inputs.report(images))
}

/// One resolved image unit: a module's label paired with its fully-rendered
/// [`ImageRequest`], resolved once during GATHER so the streamed per-unit phase
/// borrows neither the providers nor VCS readers.
struct ImageUnit {
    /// Stable unit id (the module's canonical key string).
    module_key: String,
    /// The resolved image request (name/tag/registries/sign).
    request: ImageRequest,
}

/// The shared prerequisites for `release image`, resolved once by
/// [`ImageInputs::gather`] and shared across every per-unit run.
pub struct ImageInputs {
    /// Whether this run is a mutation-free `--dry-run` preview.
    preview: bool,
    /// The project root each image is built and resolved relative to.
    project_root: PathBuf,
    /// The resolved image units, one per module declaring `[…release.image]`.
    units: Vec<ImageUnit>,
}

impl ImageInputs {
    /// Resolve every releasable module's image request once, failing closed when
    /// no module declares an image phase so the run never streams zero units.
    ///
    /// # Errors
    /// Fails closed with a typed error when no module declares an image block,
    /// and propagates configuration/discovery/graph failures and
    /// template-render failures.
    pub fn gather(
        request: &PlanRequest,
        document: &Document,
        providers: &[&dyn Provider],
        readers: &toven_core::federation::baseline::MemberVcsReaders<'_>,
        options: ImageOptions,
        reporter: &mut dyn Reporter,
    ) -> AppResult<Self> {
        let locator = PathDriverLocator::new();
        let context = prepare_front(
            &request.project_root,
            document,
            providers,
            &locator,
            reporter,
        )?;
        let targets = release_targets(&context, readers)?;
        let settings = resolve_release_settings(&context, &targets)?;

        let mut units = Vec::new();
        for (module_key, request) in resolved_image_requests(&context, &targets, &settings)? {
            units.push(ImageUnit {
                module_key,
                request,
            });
        }
        if units.is_empty() {
            return Err(AppError::invalid_input(
                "release.image",
                "no module declares an image phase; add a […release.image] block to the module \
                 that ships a container image",
            ));
        }
        units.sort_by(|left, right| left.module_key.cmp(&right.module_key));

        Ok(Self {
            preview: options.dry_run,
            project_root: request.project_root.as_path().to_path_buf(),
            units,
        })
    }

    /// Look up a resolved image unit by its unit id.
    fn unit(&self, id: &str) -> Option<&ImageUnit> {
        self.units.iter().find(|unit| unit.module_key == id)
    }

    /// Whether this run is a mutation-free `--dry-run` preview.
    #[must_use]
    pub const fn preview(&self) -> bool {
        self.preview
    }

    /// The engine unit graph: one independent (edgeless) unit per module that
    /// declares an image phase, so the engine schedules them bounded-parallel.
    #[must_use]
    pub fn units(&self) -> Vec<UnitSpec> {
        self.units
            .iter()
            .map(|unit| UnitSpec::new(unit.module_key.clone(), Vec::<String>::new()))
            .collect()
    }

    /// Assemble the terminal report from the per-module outcomes — the
    /// post-stream aggregate.
    #[must_use]
    pub fn report(&self, mut images: Vec<ImageModuleOutcome>) -> ImageReport {
        images.sort_by(|left, right| left.module.cmp(&right.module));
        ImageReport {
            preview: self.preview,
            images,
        }
    }
}

/// Build/push/sign or preview one module's image — the pure per-unit compute
/// over the gathered [`ImageInputs`], with the [`ImagePhase`] port call as its
/// only I/O.
fn image_for(
    inputs: &ImageInputs,
    image_phase: &dyn ImagePhase,
    unit: &ImageUnit,
) -> AppResult<ImageModuleOutcome> {
    if inputs.preview {
        preview_image(
            &inputs.project_root,
            image_phase,
            &unit.module_key,
            &unit.request,
        )
    } else {
        publish_one(
            &inputs.project_root,
            image_phase,
            &unit.module_key,
            &unit.request,
        )
    }
}

/// The `release image` per-unit operation on the shared runtime engine.
///
/// GATHER resolves each module's image request once into [`ImageInputs`]; each
/// unit streams one module's build/push/sign (or preview). The [`ImagePhase`]
/// call is synchronous port work, so each unit runs on a blocking thread
/// ([`tokio::task::spawn_blocking`]) to let the engine schedule the modules
/// bounded-parallel.
pub struct ImageOperation {
    inputs: Arc<ImageInputs>,
    image_phase: Arc<dyn ImagePhase>,
}

impl ImageOperation {
    /// Wrap gathered inputs and the injected image phase as a runnable
    /// operation.
    #[must_use]
    pub fn new(inputs: ImageInputs, image_phase: Arc<dyn ImagePhase>) -> Self {
        Self {
            inputs: Arc::new(inputs),
            image_phase,
        }
    }

    /// Share the gathered inputs so the CLI can title its output with the
    /// preview flag and assemble the terminal aggregate.
    #[must_use]
    pub fn inputs(&self) -> Arc<ImageInputs> {
        Arc::clone(&self.inputs)
    }
}

#[async_trait]
impl UnitOperation for ImageOperation {
    type Shared = Arc<ImageInputs>;
    type Outcome = ImageModuleOutcome;

    async fn gather(&self) -> AppResult<Self::Shared> {
        Ok(Arc::clone(&self.inputs))
    }

    async fn run(
        &self,
        shared: &Self::Shared,
        unit_id: &str,
        _cancel: CancellationToken,
    ) -> AppResult<Completed<Self::Outcome>> {
        let shared = Arc::clone(shared);
        let image_phase = Arc::clone(&self.image_phase);
        let id = unit_id.to_string();
        let outcome = tokio::task::spawn_blocking(move || {
            let unit = shared.unit(&id).ok_or_else(|| {
                AppError::new(
                    rskit_errors::ErrorCode::Internal,
                    format!("unknown image unit '{id}'"),
                )
            })?;
            image_for(&shared, image_phase.as_ref(), unit)
        })
        .await
        .map_err(AppError::internal)??;
        Ok(Completed::succeeded(outcome))
    }
}

/// Build the `release image` operation and its engine unit graph.
///
/// # Errors
/// Propagates GATHER failures (configuration/discovery/graph, template render,
/// and the fail-closed empty-image-set check).
pub fn image_operation(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    readers: &toven_core::federation::baseline::MemberVcsReaders<'_>,
    image_phase: Arc<dyn ImagePhase>,
    options: ImageOptions,
    reporter: &mut dyn Reporter,
) -> AppResult<(ImageOperation, Vec<UnitSpec>)> {
    let inputs = ImageInputs::gather(request, document, providers, readers, options, reporter)?;
    let units = inputs.units();
    Ok((ImageOperation::new(inputs, image_phase), units))
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
pub(in crate::artifacts) fn resolved_image_requests(
    context: &toven_core::plan::PlanContext,
    targets: &crate::ReleaseTargets,
    settings: &std::collections::BTreeMap<toven_model::ModuleKey, crate::ResolvedReleaseSettings>,
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

        let version = target.declared_version_required(module)?;
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
    let references = request.references();
    let primary = references.first().ok_or_else(|| {
        AppError::invalid_input(
            "release.image.registry",
            "no registry configured for the image preview",
        )
    })?;
    let primary_digest = image_phase.resolve_digest(project_root, primary)?;
    let mut all_present = primary_digest.is_some();
    for reference in references.iter().skip(1) {
        let Some(mirror_digest) = image_phase.resolve_digest(project_root, reference)? else {
            all_present = false;
            continue;
        };
        let Some(primary_digest) = &primary_digest else {
            return Err(divergent_preview_digest(
                primary,
                None,
                reference,
                &mirror_digest,
            ));
        };
        if &mirror_digest != primary_digest {
            return Err(divergent_preview_digest(
                primary,
                Some(primary_digest),
                reference,
                &mirror_digest,
            ));
        }
    }
    let status = if all_present {
        ImagePhaseStatus::AlreadyPresent
    } else {
        ImagePhaseStatus::WouldPush
    };
    Ok(ImageModuleOutcome {
        module: module_key.to_string(),
        references,
        digest: primary_digest,
        signed: request.sign,
        status,
    })
}

/// Report inconsistent primary/mirror registry state during mutation-free
/// preview rather than hiding it behind a successful-looking digest.
fn divergent_preview_digest(
    primary: &str,
    primary_digest: Option<&String>,
    mirror: &str,
    mirror_digest: &str,
) -> AppError {
    let primary_value = primary_digest.map_or("missing", String::as_str);
    AppError::invalid_input(
        "release.image",
        format!(
            "image mirror '{mirror}' resolves to digest {mirror_digest}, but primary '{primary}' \
             resolves to {primary_value}; releases are immutable, so reconcile the registry state \
             before previewing or publishing"
        ),
    )
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
