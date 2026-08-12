use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rskit_errors::{AppError, AppResult};
use rskit_fs::TempFile;
use rskit_fs::sync_io::file::read_string_bounded;
use toven_ports::{
    ImageOutcome, ImagePhase, ImageRequest, ToolInvocation, ToolOutcome, ToolRunner,
};

/// Hard bound on captured tool output (256 KiB) — a build log is terse in the
/// captured tail; this only guards against a pathological stream.
const MAX_IMAGE_OUTPUT_BYTES: usize = 256 * 1024;

/// Hard bound on the `docker buildx` metadata file read (64 KiB) — it holds a
/// small JSON object of build outputs, so this only guards a pathological file.
const MAX_IMAGE_METADATA_BYTES: u64 = 64 * 1024;

/// Timeout for a single image build/push/sign invocation. Builds and registry
/// round-trips are slower than a local command.
const IMAGE_TIMEOUT: Duration = Duration::from_mins(30);

/// A buildx/cosign-backed [`ImagePhase`].
///
/// Construction injects the shared [`ToolRunner`]. `docker buildx` and `cosign`
/// are invoked argv-only through it; the ambient registry/OIDC credentials the
/// runner provides are inherited, and no secret is placed on argv or captured.
#[derive(Clone)]
pub struct BuildxImagePhase {
    runner: Arc<dyn ToolRunner>,
    timeout: Duration,
}

impl BuildxImagePhase {
    /// Construct a buildx image phase driven through `runner` with the default
    /// per-invocation timeout.
    #[must_use]
    pub fn new(runner: Arc<dyn ToolRunner>) -> Self {
        Self {
            runner,
            timeout: IMAGE_TIMEOUT,
        }
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
        let outcome = self.invoke(root, "docker", inspect_argv(reference))?;
        if outcome.succeeded() {
            let digest = outcome.stdout.trim().to_string();
            if digest.is_empty() {
                return Ok(None);
            }
            return Ok(Some(digest));
        }
        if image_not_found(&outcome) {
            return Ok(None);
        }
        Err(process_failure("release.image.inspect", "docker", &outcome))
    }
}

/// Whether `docker buildx imagetools inspect` reported the specific
/// tag-missing condition that previews and immutable publish guards treat as
/// absence. Auth, network, timeout, and malformed invocations fail closed.
fn image_not_found(outcome: &ToolOutcome) -> bool {
    let output = format!("{}\n{}", outcome.stdout, outcome.stderr).to_ascii_lowercase();
    output.contains("not found")
        || output.contains("manifest unknown")
        || output.contains("name unknown")
}

/// Convert a non-zero tool outcome into a typed fail-closed error with bounded
/// captured diagnostics.
fn process_failure(field: &str, program: &str, outcome: &ToolOutcome) -> AppError {
    let mut message = format!(
        "{program} exited with code {}; refusing to treat the result as absent",
        outcome
            .exit_code
            .map_or_else(|| "unknown".to_string(), |code| code.to_string())
    );
    let stderr = outcome.stderr.trim();
    if !stderr.is_empty() {
        message.push_str(": ");
        message.push_str(stderr);
    }
    AppError::new(rskit_errors::ErrorCode::Internal, message)
        .with_detail("field", field)
        .with_detail("program", program)
}

impl BuildxImagePhase {
    /// Build the captured, bounded [`ToolInvocation`] for an argv-only
    /// `docker`/`cosign` invocation rooted at `root`, and run it through the
    /// shared runner, returning the classified outcome without gating.
    fn invoke(&self, root: &Path, program: &str, argv: Vec<String>) -> AppResult<ToolOutcome> {
        let mut full_argv = Vec::with_capacity(argv.len() + 1);
        full_argv.push(program.to_string());
        full_argv.extend(argv);
        let invocation = ToolInvocation::new(full_argv)
            .with_working_dir(root)
            .with_timeout(self.timeout)
            .with_max_output_bytes(MAX_IMAGE_OUTPUT_BYTES);
        self.runner.run(&invocation)
    }

    /// Run an argv-only `docker`/`cosign` invocation rooted at `root`, failing
    /// closed on a spawn or non-zero exit.
    fn run(&self, root: &Path, program: &str, argv: Vec<String>) -> AppResult<()> {
        self.invoke(root, program, argv)?
            .require_success(&format!("image tool `{program}`"))
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
pub(super) fn parse_metadata_digest(text: &str) -> AppResult<String> {
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
pub(super) fn buildx_build_argv(
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
pub(super) fn buildx_push_argv(
    request: &ImageRequest,
    references: &[String],
) -> AppResult<Vec<String>> {
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
pub(super) fn inspect_argv(reference: &str) -> Vec<String> {
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
