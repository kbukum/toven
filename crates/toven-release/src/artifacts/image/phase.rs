use std::path::{Path, PathBuf};

use rskit_errors::{AppError, AppResult};
use rskit_util::Template;
use toven_core::config::Document;
use toven_core::federation::resolve::PathDriverLocator;
use toven_core::plan::{PlanRequest, prepare_front};
use toven_ports::{ImageOutcome, ImagePhase, ImageRequest, Provider, ReleaseVar, Reporter};

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
