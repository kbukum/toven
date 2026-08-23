//! `CargoRegistryTarget` — the cargo release-adapter sliver (crates.io by
//! default, or a named alternate registry).
//!
//! Owns the ecosystem-specific ~10% of release: reading a module's declared
//! version from its `Cargo.toml`, applying one atomic version mutation (own
//! version + intra-project dependency-floor rewrites) with format-preserving
//! `toml_edit`, and the registry-facing trio (`published_versions`, `package`,
//! `publish`) routed through `cargo` via `rskit-process` with bounded output
//! and a hard timeout. Each method returns typed data or a typed error — never
//! a success-shaped fallback.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::sync_io::dir::create_all;
use rskit_fs::sync_io::file::{
    exists, move_file, read_string_bounded, remove_if_exists, write_atomic_replace,
};
use rskit_fs::{canonicalize, safe_join};
use rskit_util::time::parse_rfc2822;
use rskit_util::{Template, TemplatePart};
use rskit_version::semver::Version;
use toml_edit::{DocumentMut, Item, value};
use toven_model::{Module, RepoPath};
use toven_ports::{
    Artifact, BaselineSourceConfig, ForwardEnvAs, ManifestMutator, Packager, PublishOutcome,
    Publisher, RegistryCadence, ReleaseCredentials, ReleaseDefaults, ReleaseDefaultsSource,
    ReleaseMutation, ReleaseVar, SbomProducer, TagGrammar, TagMode, TagScheme, ToolInvocation,
    ToolOutcome, ToolRunner, VersionSource, Visibility,
};

/// Hard bound on a `Cargo.toml` read (4 MiB) — manifests are tiny; this only
/// guards against a pathological file.
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;

/// Temp-file prefix for atomic manifest rewrites.
const MANIFEST_TEMP_PREFIX: &str = "toven-cargo-manifest";

/// Default Rust release tag template.
const DEFAULT_TAG_FORMAT: &str = "{ecosystem}/{module}@{version}";

/// The environment variable cargo reads for the registry publish token. A
/// configured `token_env` is resolved to its value and injected under this name
/// on the `cargo publish` child, so the secret never appears on argv.
const CARGO_REGISTRY_TOKEN_ENV: &str = "CARGO_REGISTRY_TOKEN";

/// Sentinel used to split a rendered tag template into version prefix/suffix.
const VERSION_SENTINEL: &str = "\u{1f}TOVEN_VERSION_SENTINEL\u{1f}";

/// Maximum retained stdout/stderr for cargo registry commands (64 KiB each).
const MAX_CARGO_OUTPUT_BYTES: usize = 64 * 1024;

/// Maximum retained `cargo metadata` output (16 MiB) — large enough for big
/// workspaces, bounded so a runaway process cannot exhaust memory.
const MAX_METADATA_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

/// Timeout for a single cargo registry-facing command.
const CARGO_COMMAND_TIMEOUT: Duration = Duration::from_mins(2);

/// The cargo registry release target for the Rust ecosystem (crates.io by
/// default, or a named alternate registry).
///
/// Resolves each module's repo-relative `manifest` against the process working
/// directory (the engine runs from the repository root).
#[derive(Clone)]
pub struct CargoRegistryTarget {
    runner: Arc<dyn ToolRunner>,
}

impl CargoRegistryTarget {
    /// Construct the crates.io target.
    #[must_use]
    pub fn new(runner: Arc<dyn ToolRunner>) -> Self {
        Self { runner }
    }

    /// Resolve a module's repo-relative manifest to an absolute path under the
    /// current working directory.
    fn manifest_path(module: &Module) -> AppResult<PathBuf> {
        let manifest = module.manifest.as_ref().ok_or_else(|| {
            AppError::invalid_input(
                "module.manifest",
                format!("module '{}' has no manifest to release", module.id),
            )
        })?;
        let cwd = Self::working_root()?;
        let resolved = safe_join(&cwd, manifest.as_path()).map_err(|error| {
            AppError::invalid_input(
                "module.manifest",
                format!(
                    "manifest '{}' escapes the working directory: {error}",
                    manifest.as_path().display()
                ),
            )
        })?;
        // `module.manifest` is untrusted input from discovery/config: fail fast with a
        // typed input error rather than spawning cargo against a path that does not
        // exist on disk.
        if !exists(&resolved)? {
            return Err(AppError::invalid_input(
                "module.manifest",
                format!(
                    "manifest '{}' does not exist for module '{}'",
                    resolved.display(),
                    module.id
                ),
            ));
        }
        Ok(resolved)
    }

    /// Resolve the cargo target directory for `manifest`, honoring
    /// `CARGO_TARGET_DIR`, `.cargo/config.toml` `build.target-dir`, and the
    /// workspace layout — `cargo metadata` reports the effective directory.
    fn target_directory(&self, manifest: &Path) -> AppResult<PathBuf> {
        let output = self.cargo_metadata_command(Self::working_root()?, manifest)?;
        output.require_read_success("metadata tool `cargo`")?;
        let metadata = rskit_codec::decode::<cargo_metadata::Metadata>(
            &rskit_codec::JsonCodec::default(),
            &output.stdout,
        )
        .map_err(|error| {
            AppError::new(
                ErrorCode::InvalidFormat,
                format!(
                    "failed to parse `cargo metadata` output for '{}'",
                    manifest.display()
                ),
            )
            .with_cause(error)
        })?;
        Ok(metadata.target_directory.into_std_path_buf())
    }

    /// The trust boundary for manifest resolution: the process working
    /// directory (the engine runs from the repository root).
    fn working_root() -> AppResult<PathBuf> {
        std::env::current_dir().map_err(|error| {
            AppError::new(ErrorCode::Internal, "failed to read current directory").with_cause(error)
        })
    }

    fn cargo<I>(&self, working_dir: PathBuf, args: I) -> AppResult<ToolOutcome>
    where
        I: IntoIterator<Item = String>,
    {
        self.cargo_with_env(working_dir, args, None, MAX_CARGO_OUTPUT_BYTES)
    }

    /// Run `cargo`, bounded and timed-out, optionally forwarding the registry
    /// token to the child under the name cargo reads (`CARGO_REGISTRY_TOKEN`)
    /// *by ambient-variable name* — the runner resolves the value at run time,
    /// so the secret is never placed on argv and never crosses the seam by
    /// value. The inherited parent environment is preserved.
    fn cargo_with_env<I>(
        &self,
        working_dir: PathBuf,
        args: I,
        secret: Option<ForwardEnvAs>,
        max_output_bytes: usize,
    ) -> AppResult<ToolOutcome>
    where
        I: IntoIterator<Item = String>,
    {
        let mut full_argv = vec!["cargo".to_string()];
        full_argv.extend(args);
        let mut invocation = ToolInvocation::new(full_argv)
            .with_working_dir(working_dir)
            .with_timeout(CARGO_COMMAND_TIMEOUT)
            .with_max_output_bytes(max_output_bytes);
        if let Some(mapping) = secret {
            invocation = invocation.with_forward_env_as(vec![mapping]);
        }
        self.runner.run(&invocation)
    }

    /// Run `cargo metadata --no-deps` for `manifest`, bounded and timed-out, to
    /// read the effective target directory. `metadata` output can be large for big
    /// workspaces, so this uses a wider output bound than the registry commands.
    fn cargo_metadata_command(
        &self,
        working_dir: PathBuf,
        manifest: &Path,
    ) -> AppResult<ToolOutcome> {
        self.cargo_with_env(
            working_dir,
            [
                "metadata".to_string(),
                "--no-deps".to_string(),
                "--format-version".to_string(),
                "1".to_string(),
                "--manifest-path".to_string(),
                manifest.display().to_string(),
            ],
            None,
            MAX_METADATA_OUTPUT_BYTES,
        )
    }

    fn remove_sbom_strays(
        &self,
        manifest: &Path,
        stem: &str,
        preserve: Option<&Path>,
    ) -> AppResult<()> {
        remove_stray_sbom_files(&self.workspace_member_dirs(manifest)?, stem, preserve)
    }

    /// The directory of each workspace member manifest, resolved via
    /// `cargo metadata --no-deps` (which reports members only).
    fn workspace_member_dirs(&self, manifest: &Path) -> AppResult<Vec<PathBuf>> {
        let output = self.cargo_metadata_command(Self::working_root()?, manifest)?;
        output.require_read_success("metadata tool `cargo`")?;
        let metadata = rskit_codec::decode::<cargo_metadata::Metadata>(
            &rskit_codec::JsonCodec::default(),
            &output.stdout,
        )
        .map_err(|error| {
            AppError::new(
                ErrorCode::InvalidFormat,
                format!(
                    "failed to parse `cargo metadata` output for '{}'",
                    manifest.display()
                ),
            )
            .with_cause(error)
        })?;
        let workspace_root = metadata.workspace_root.as_std_path();
        Ok(metadata
            .packages
            .iter()
            .filter(|package| metadata.workspace_members.contains(&package.id))
            .filter_map(|package| package.manifest_path.parent())
            .filter(|dir| dir.starts_with(workspace_root))
            .map(|dir| dir.as_std_path().to_path_buf())
            .collect())
    }
}

impl VersionSource for CargoRegistryTarget {
    fn declared_version(&self, module: &Module) -> AppResult<Option<Version>> {
        let root = Self::working_root()?;
        let path = Self::manifest_path(module)?;
        let text = read_string_bounded(&path, MAX_MANIFEST_BYTES)?;
        // A Cargo manifest always declares a version, so a Rust crate always has
        // a concrete declared version.
        read_declared_version(&text, &path, &root).map(Some)
    }

    fn published_versions(&self, module: &Module) -> AppResult<Vec<Version>> {
        let package = package_name(module);
        // `cargo search` reports only the single latest version of a crate, so this is
        // the best-effort "latest published" set the port contract allows — the publish
        // loop's `AlreadyPublished` classification is the authoritative idempotency
        // backstop for older versions.
        let output = self.cargo(
            Self::working_root()?,
            [
                "search".to_string(),
                package.clone(),
                "--limit".to_string(),
                "1".to_string(),
            ],
        )?;
        output.require_success("registry query tool `cargo`")?;
        Ok(parse_cargo_search_versions(&package, &output.stdout))
    }

    fn version_in_manifest(&self, manifest: &str) -> AppResult<Option<Version>> {
        version_in_manifest_contents(manifest)
    }
}

impl TagGrammar for CargoRegistryTarget {
    fn tag_scheme(&self, module: &Module, tag_format: Option<&str>) -> AppResult<TagScheme> {
        tag_scheme(module, tag_format.unwrap_or(DEFAULT_TAG_FORMAT))
    }
}

impl ReleaseDefaultsSource for CargoRegistryTarget {
    fn release_defaults(&self) -> ReleaseDefaults {
        // crates.io is the registry and git tags are traceability only: anchor
        // change detection on `max(registry, umbrella-tag)` and cut per-crate
        // tags for traceability alongside the one umbrella `v{version}` tag. The
        // engine degrades the umbrella-anchored default to a per-module baseline
        // for a workspace that declares no umbrella module.
        ReleaseDefaults::new(BaselineSourceConfig::RegistryUmbrella, TagMode::Both)
    }
}

impl Packager for CargoRegistryTarget {
    fn package(&self, module: &Module) -> AppResult<Artifact> {
        let path = Self::manifest_path(module)?;
        let output = self.cargo(
            Self::working_root()?,
            [
                "package".to_string(),
                "--manifest-path".to_string(),
                path.display().to_string(),
                "--allow-dirty".to_string(),
            ],
        )?;
        output.require_success("release tool `cargo`")?;

        let artifact = self.target_directory(&path)?.join("package").join(format!(
            "{}-{}.crate",
            package_name(module),
            self.declared_version_required(module)?
        ));
        Ok(Artifact::new(artifact))
    }
}

impl ManifestMutator for CargoRegistryTarget {
    fn apply_release(
        &self,
        module: &Module,
        mutation: &ReleaseMutation,
    ) -> AppResult<Vec<RepoPath>> {
        let manifest = module.manifest.clone().ok_or_else(|| {
            AppError::invalid_input(
                "module.manifest",
                format!("module '{}' has no manifest to release", module.id),
            )
        })?;
        let working_root = Self::working_root()?;
        let path = Self::manifest_path(module)?;
        let text = read_string_bounded(&path, MAX_MANIFEST_BYTES)?;
        let mut doc = parse_manifest(&text, &path)?;

        // A version bump routes to whichever manifest actually *owns* the
        // version: the member's own `[package].version` when it declares a
        // literal, or the workspace root's `[workspace.package].version` when
        // the member inherits it (`version.workspace = true`). Resolving this
        // before mutating keeps the writer's source of truth identical to the
        // reader's (`read_declared_version`), so `release plan`/`status` and the
        // applied mutation can never diverge.
        let version_site = match &mutation.new_version {
            Some(_) => Some(resolve_version_site(&doc, &path, &working_root)?),
            None => None,
        };

        // Dependency-floor rewrites are always per-member (`[dependencies]` &c.).
        for (dependency, floor) in &mutation.dep_floor_updates {
            set_dependency_floor(&mut doc, &dependency.name, floor);
        }

        if let (Some(new_version), Some(site)) = (&mutation.new_version, version_site.as_ref()) {
            match site {
                // Literal member version, or a root that is both workspace and
                // package (the inherited version lives in this same manifest):
                // bump in place.
                VersionSite::OwnPackage => set_package_version(&mut doc, new_version, &path)?,
                VersionSite::Workspace(owner) if owner == &path => {
                    set_workspace_package_version(&mut doc, new_version, &path)?;
                }
                // Inherited from a *separate* workspace-root manifest: this
                // member's version is not written here at all.
                VersionSite::Workspace(_) => {}
            }
        }

        let mut changed = Vec::new();
        let rewritten = doc.to_string();
        if rewritten != text {
            write_atomic_replace(&path, rewritten.as_bytes(), MANIFEST_TEMP_PREFIX)?;
            changed.push(manifest);
        }

        // Route an inherited version bump to the owning workspace-root manifest
        // exactly once. Many members of a single-version workspace resolve to
        // the same root; the engine dedupes the staged path set, and the write
        // is idempotent, so a repeated bump to the same version is a no-op.
        if let (Some(new_version), Some(VersionSite::Workspace(owner))) =
            (&mutation.new_version, version_site.as_ref())
            && owner != &path
        {
            let owner_text = read_string_bounded(owner, MAX_MANIFEST_BYTES)?;
            let owner_rewritten = apply_workspace_root_version(&owner_text, owner, new_version)?;
            if owner_rewritten != owner_text {
                write_atomic_replace(owner, owner_rewritten.as_bytes(), MANIFEST_TEMP_PREFIX)?;
            }
            changed.push(workspace_root_repo_path(owner, &working_root)?);
        }

        Ok(changed)
    }
}

impl Publisher for CargoRegistryTarget {
    fn publish(
        &self,
        module: &Module,
        _artifact: &Artifact,
        credentials: &ReleaseCredentials,
        visibility: Visibility,
    ) -> AppResult<PublishOutcome> {
        let registry = credentials.registry();
        // crates.io (the cargo default) is a public-only registry: every
        // published version is world readable. Fail closed rather than publish a
        // version a maintainer asked to keep private/internal to a registry that
        // cannot honor it. A named alternate registry is assumed to support the
        // requested exposure (its own access controls define it), so the publish
        // proceeds to that registry.
        if is_default_registry(registry) && !visibility.is_public() {
            return Err(AppError::invalid_input(
                "release.visibility",
                format!(
                    "crates.io only publishes public versions, but module '{}' requests \
                     visibility = {}; publish it to a registry that supports that exposure or set \
                     visibility = public",
                    module.key(),
                    visibility.as_str(),
                ),
            ));
        }
        let path = Self::manifest_path(module)?;
        // Resolve the registry-token *mapping* here, at the toolchain boundary —
        // which ambient variable holds the token and which name cargo reads it
        // under — and hand only those names across the seam. The runner resolves
        // the value at run time, so it never touches argv or engine memory.
        // `None` lets cargo resolve its ambient credential as usual.
        let token = registry_token_injection(credentials, rskit_util::env::get_non_empty)?;
        let output = self.cargo_with_env(
            Self::working_root()?,
            publish_argv(&path, registry),
            token,
            MAX_CARGO_OUTPUT_BYTES,
        )?;
        classify_publish(self, module, &output)
    }
}

impl SbomProducer for CargoRegistryTarget {
    fn sbom(&self, module: &Module, out_dir: &Path) -> AppResult<Option<Artifact>> {
        let manifest = Self::manifest_path(module)?;
        create_all(out_dir)?;
        let stem = package_name(module);
        let output = self.cargo(Self::working_root()?, sbom_argv(&manifest, &stem))?;
        output.require_success("sbom tool `cargo`")?;
        // `cargo cyclonedx` ignores the process working directory and writes its
        // output next to the manifest; the filename suffix is version-specific (0.5.x
        // writes `<stem>.json`). Locate whichever file it produced, then move it into
        // `out_dir` under Toven's canonical `<stem>.cdx.json` name so the manifest tree
        // is left clean and callers get a stable artifact path.
        let produced =
            first_existing(&sbom_output_candidates(&manifest, &stem))?.ok_or_else(|| {
                AppError::new(
                    ErrorCode::Internal,
                    format!(
                        "cargo cyclonedx reported success but wrote no SBOM next to '{}'",
                        manifest.display()
                    ),
                )
            })?;
        let artifact = out_dir.join(format!("{stem}.{SBOM_FILE_SUFFIX}"));
        move_file(&produced, &artifact)?;
        // `cargo cyclonedx` resolves the whole workspace and writes a copy of the
        // SBOM next to *every* member manifest, not just the requested one. Remove
        // those sibling copies so the manifest tree is left clean and only the
        // artifact under `out_dir` remains.
        self.remove_sbom_strays(&manifest, &stem, Some(&artifact))?;
        Ok(Some(Artifact::new(artifact)))
    }
}

fn tag_scheme(module: &Module, template: &str) -> AppResult<TagScheme> {
    let template = Template::parse(template, ReleaseVar::ALL).map_err(|error| {
        AppError::invalid_input(
            "release.tag_format",
            format!("invalid release template: {error}"),
        )
        .with_cause(error)
    })?;
    let version_count = template
        .parts()
        .iter()
        .filter(|part| matches!(part, TemplatePart::Placeholder(ReleaseVar::Version)))
        .count();
    if version_count != 1 {
        return Err(AppError::invalid_input(
            "release.tag_format",
            "release tag template must contain exactly one {version} placeholder",
        ));
    }
    if template
        .parts()
        .iter()
        .any(|part| matches!(part, TemplatePart::Placeholder(ReleaseVar::Channel)))
    {
        // The prerelease channel is already carried inside `{version}` (e.g.
        // `1.0.0-rc.1`), so a static tag prefix/suffix cannot fill `{channel}`; reject
        // it instead of rendering it empty.
        return Err(AppError::invalid_input(
            "release.tag_format",
            "release tag template must not contain {channel}: the prerelease channel is part of {version}",
        ));
    }
    let rendered = template
        .render_with(|placeholder| {
            Ok::<_, AppError>(match placeholder {
                ReleaseVar::Version => VERSION_SENTINEL.to_string(),
                ReleaseVar::Ecosystem => module.id.ecosystem.as_str().to_string(),
                ReleaseVar::Module => module.id.name.clone(),
                _ => String::new(),
            })
        })
        .map_err(|error| {
            AppError::invalid_input(
                "release.tag_format",
                format!("invalid release template: {error}"),
            )
            .with_cause(error)
        })?;
    if rendered.matches(VERSION_SENTINEL).count() != 1 {
        return Err(AppError::invalid_input(
            "release.tag_format",
            "release tag template rendered an ambiguous version marker",
        ));
    }
    let (prefix, suffix) = rendered.split_once(VERSION_SENTINEL).ok_or_else(|| {
        AppError::invalid_input(
            "release.tag_format",
            "release tag template must contain exactly one {version} placeholder",
        )
    })?;
    Ok(TagScheme::new(prefix, suffix))
}

/// `CycloneDX` SBOM output suffix for the JSON format (`<stem>.cdx.json`).
const SBOM_FILE_SUFFIX: &str = "cdx.json";

/// Candidate on-disk paths `cargo cyclonedx` may write for `stem`, in priority
/// order, resolved next to `manifest` (the tool ignores the process working
/// directory). The suffix is version-specific — 0.5.x writes `<stem>.json`,
/// while other versions emit `<stem>.cdx.json` — so both are probed.
fn sbom_output_candidates(manifest: &Path, stem: &str) -> Vec<PathBuf> {
    let dir = manifest.parent().unwrap_or_else(|| Path::new("."));
    [SBOM_FILE_SUFFIX, "json"]
        .into_iter()
        .map(|suffix| dir.join(format!("{stem}.{suffix}")))
        .collect()
}

/// Return the first path in `candidates` that exists on disk, or `None`.
fn first_existing(candidates: &[PathBuf]) -> AppResult<Option<PathBuf>> {
    for candidate in candidates {
        if exists(candidate)? {
            return Ok(Some(candidate.clone()));
        }
    }
    Ok(None)
}

/// Remove the stray `<stem>` SBOM files `cargo cyclonedx` wrote next to sibling
/// workspace members. The requested module's own copy has already been moved
/// into `out_dir`, so every remaining `<stem>.{cdx.json,json}` under a member
/// directory is a redundant sibling copy safe to delete.
/// Delete every `<stem>.{cdx.json,json}` file found directly in `dirs`.
fn remove_stray_sbom_files(dirs: &[PathBuf], stem: &str, preserve: Option<&Path>) -> AppResult<()> {
    for dir in dirs {
        for suffix in [SBOM_FILE_SUFFIX, "json"] {
            let candidate = dir.join(format!("{stem}.{suffix}"));
            if should_remove_sbom(&candidate, preserve)? {
                remove_if_exists(&candidate)?;
            }
        }
    }
    Ok(())
}

fn should_remove_sbom(candidate: &Path, preserve: Option<&Path>) -> AppResult<bool> {
    if !exists(candidate)? {
        return Ok(false);
    }
    let Some(preserve) = preserve else {
        return Ok(true);
    };
    Ok(canonicalize(candidate)? != canonicalize(preserve)?)
}

/// Build the argv-only `cargo cyclonedx` invocation for `manifest`.
///
/// cyclonedx ignores the process working directory and writes next to each
/// member manifest, so `--override-filename` pins the output stem
/// deterministically instead of relying on the crate name.
fn sbom_argv(manifest: &Path, stem: &str) -> Vec<String> {
    vec![
        "cyclonedx".to_string(),
        "--manifest-path".to_string(),
        manifest.display().to_string(),
        "--format".to_string(),
        "json".to_string(),
        "--override-filename".to_string(),
        stem.to_string(),
    ]
}

/// Resolve the registry-token environment mapping for a publish attempt.
///
/// Given the publish `credentials` and an environment accessor `env` (a
/// variable name → its non-empty value), return the [`ForwardEnvAs`] mapping the
/// runner uses to forward the token to cargo: the configured ambient
/// `token_env` as the `source`, and the name cargo reads (`CARGO_REGISTRY_TOKEN`
/// for crates.io, or `CARGO_REGISTRIES_<NAME>_TOKEN` for a named alternate
/// registry) as the `child`. Returns `None` when no `token_env` is configured
/// (cargo falls back to its ambient credential). The accessor is consulted only
/// to fail closed on a configured-but-absent/empty variable — the maintainer
/// named an explicit credential source that is not present, so error rather
/// than silently attempt an unauthenticated publish — and the value is
/// **discarded**, never returned, so the secret never crosses the seam by
/// value. The accessor is a parameter so the resolution logic is unit-tested
/// without touching the real process environment.
fn registry_token_injection<F>(
    credentials: &ReleaseCredentials,
    env: F,
) -> AppResult<Option<ForwardEnvAs>>
where
    F: Fn(&str) -> Option<String>,
{
    let Some(name) = credentials.registry_token_env() else {
        return Ok(None);
    };
    if env(name).is_none() {
        return Err(AppError::invalid_input(
            "release.token_env",
            format!(
                "release.token_env names environment variable '{name}', but it is unset or \
                 empty; export the registry token there before publishing"
            ),
        ));
    }
    Ok(Some(ForwardEnvAs::new(
        name,
        cargo_token_env_name(credentials.registry()),
    )))
}

/// Whether `registry` names the cargo default registry (crates.io) rather than
/// a named alternate. `None` and crates.io's canonical names are the default;
/// any other value selects a named alternate registry.
fn is_default_registry(registry: Option<&str>) -> bool {
    matches!(registry, None | Some("crates-io" | "crates.io"))
}

/// Build the `cargo publish` argv for a module, routing to a named alternate
/// registry via `--registry <name>` when one is configured (the cargo default
/// registry, crates.io, adds no flag).
fn publish_argv(manifest: &Path, registry: Option<&str>) -> Vec<String> {
    let mut argv = vec![
        "publish".to_string(),
        "--manifest-path".to_string(),
        manifest.display().to_string(),
        "--allow-dirty".to_string(),
    ];
    if !is_default_registry(registry)
        && let Some(name) = registry
    {
        argv.push("--registry".to_string());
        argv.push(name.to_string());
    }
    argv
}

/// The cargo environment-variable name that carries the publish token for the
/// selected registry: `CARGO_REGISTRY_TOKEN` for the default registry, or
/// `CARGO_REGISTRIES_<NAME>_TOKEN` for a named alternate registry (cargo's
/// config-env convention: the registry name uppercased with every
/// non-alphanumeric byte replaced by `_`).
fn cargo_token_env_name(registry: Option<&str>) -> String {
    match registry {
        Some(name) if !is_default_registry(Some(name)) => {
            format!("CARGO_REGISTRIES_{}_TOKEN", cargo_registry_env_key(name))
        }
        _ => CARGO_REGISTRY_TOKEN_ENV.to_string(),
    }
}

/// Uppercase a registry name and replace every non-alphanumeric character with
/// `_`, matching cargo's `[registries.<name>]` config-env key derivation.
fn cargo_registry_env_key(registry: &str) -> String {
    registry
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn package_name(module: &Module) -> String {
    module
        .package
        .clone()
        .unwrap_or_else(|| module.id.name.clone())
}

fn parse_cargo_search_versions(package: &str, stdout: &str) -> Vec<Version> {
    stdout
        .lines()
        .filter_map(|line| {
            let (name, rest) = line.split_once('=')?;
            if name.trim() != package {
                return None;
            }
            let raw = rest.trim().trim_matches('"').split('"').next()?;
            Version::parse(raw).ok()
        })
        .collect()
}

fn classify_publish(
    target: &CargoRegistryTarget,
    module: &Module,
    output: &ToolOutcome,
) -> AppResult<PublishOutcome> {
    if output.succeeded() {
        return Ok(PublishOutcome::Published);
    }

    let combined = format!("{}\n{}", output.stdout, output.stderr).to_ascii_lowercase();
    if combined.contains("already uploaded")
        || combined.contains("already exists")
        || combined.contains("already been uploaded")
    {
        return Ok(PublishOutcome::AlreadyPublished);
    }

    if combined.contains("429")
        || combined.contains("rate limit")
        || combined.contains("too many requests")
    {
        // crates.io applies a stricter cadence to a brand-new crate name than to a new
        // version of an existing one. Treat "no versions on the registry" as a first
        // publish; a failed lookup falls back to the existing-name cadence.
        let is_new_release = target
            .published_versions(module)
            .is_ok_and(|versions| versions.is_empty());
        let now = SystemTime::now();
        // Prefer crates.io's own authoritative deadline ("try again after <HTTP-date>")
        // when it parses to a future instant; only guess the cadence when the rejection
        // carries no usable hint (or the hint is already in the past from clock skew).
        let retry_after =
            retry_after_hint(&combined, now).or_else(|| fallback_retry_after(is_new_release, now));
        return Ok(PublishOutcome::RateLimited { retry_after });
    }

    output
        .require_success("publish tool `cargo`")
        .map(|()| PublishOutcome::Published)
}

/// Extract crates.io's rate-limit deadline from a rejected `cargo publish`
/// output. crates.io renders it as `...try again after <RFC 2822 date> and
/// see...`, where the date is terminated by its `GMT` zone. Returns the instant
/// only when it parses and lies in the future — a past or absent hint yields
/// `None` so the caller falls back to the cadence rather than busy-retrying.
fn retry_after_hint(output: &str, now: SystemTime) -> Option<SystemTime> {
    // `output` is already lowercased by the caller; `parse_rfc2822` is
    // case-insensitive, and the marker/zone matching below assumes lowercase.
    let tail = output
        .split_once("try again after ")
        .map(|(_, rest)| rest)?;
    // Cut at the inclusive `gmt` zone token that closes the date.
    let end = tail.find("gmt")? + "gmt".len();
    let epoch_secs = parse_rfc2822(tail.get(..end)?)?;
    let deadline =
        SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(u64::try_from(epoch_secs).ok()?))?;
    (deadline > now).then_some(deadline)
}

fn fallback_retry_after(is_new_release: bool, now: SystemTime) -> Option<SystemTime> {
    RegistryCadence::new(Duration::from_mins(10), Duration::from_mins(1))
        .fallback_retry_after(is_new_release, now)
}

/// Parse `[package].version` from a `Cargo.toml` **body** without touching the
/// working tree, for anchoring a module on its own version at a historical
/// commit (an umbrella-tag baseline).
///
/// Returns `None` — never an error — when the body is well-formed TOML but does
/// not resolve a concrete version here: no `[package].version` (a virtual
/// workspace manifest), or a `version.workspace = true` whose owning
/// `[workspace.package].version` lives in an ancestor manifest not available
/// from this body alone. The caller falls back to the umbrella tag's own
/// version in those cases. Malformed TOML or a non-string, non-inherited
/// version is a genuine parse fault and propagates.
fn version_in_manifest_contents(text: &str) -> AppResult<Option<Version>> {
    let attribution = Path::new("Cargo.toml");
    let doc = parse_manifest(text, attribution)?;
    let Some(item) = doc
        .get("package")
        .and_then(Item::as_table_like)
        .and_then(|package| package.get("version"))
    else {
        return Ok(None);
    };
    match item.as_str() {
        Some(raw) => parse_version(raw, attribution).map(Some),
        // `version.workspace = true`: the concrete version is in the workspace
        // root manifest at the same commit, not this body — fall back rather
        // than resolve a working-tree ancestor that may differ from the commit.
        None if is_workspace_inherited(item) => Ok(None),
        None => Err(AppError::new(
            ErrorCode::InvalidFormat,
            "manifest has a [package].version that is neither a string nor \
             `version.workspace = true`",
        )),
    }
}

/// `[workspace.package] version` — first in the same manifest (root package),
/// then by walking ancestor directories from `path` for the workspace-root
/// `Cargo.toml`. The ancestor walk never crosses above `root` (the
/// working/repository-root trust boundary).
fn read_declared_version(text: &str, path: &Path, root: &Path) -> AppResult<Version> {
    let doc = parse_manifest(text, path)?;
    let version_item = doc
        .get("package")
        .and_then(Item::as_table_like)
        .and_then(|package| package.get("version"));

    let missing = || {
        AppError::new(
            ErrorCode::InvalidFormat,
            format!("manifest '{}' has no [package].version", path.display()),
        )
    };

    let item = version_item.ok_or_else(missing)?;
    item.as_str().map_or_else(
        || {
            if is_workspace_inherited(item) {
                resolve_inherited_version(&doc, path, root)
            } else {
                Err(AppError::new(
                    ErrorCode::InvalidFormat,
                    format!(
                        "manifest '{}' has a [package].version that is neither a string nor \
                         `version.workspace = true`",
                        path.display()
                    ),
                ))
            }
        },
        |raw| parse_version(raw, path),
    )
}

/// Whether a `[package].version` entry is `version.workspace = true`.
fn is_workspace_inherited(item: &Item) -> bool {
    item.as_table_like()
        .and_then(|table| table.get("workspace"))
        .and_then(Item::as_bool)
        == Some(true)
}

/// Resolve a `version.workspace = true` package version from the owning
/// workspace's `[workspace.package].version`.
///
/// Reader/writer share [`resolve_workspace_root_manifest`] so the version the
/// reader reports and the manifest the writer bumps are always the same source
/// of truth.
fn resolve_inherited_version(doc: &DocumentMut, path: &Path, root: &Path) -> AppResult<Version> {
    let (owner_path, owner_doc) = resolve_workspace_root_manifest(doc, path, root)?;
    let raw = workspace_package_version(&owner_doc).ok_or_else(|| {
        AppError::new(
            ErrorCode::InvalidFormat,
            format!(
                "workspace root '{}' has no [workspace.package].version to inherit",
                owner_path.display()
            ),
        )
    })?;
    parse_version(raw, &owner_path)
}

/// Where a member's version bump must be written.
#[derive(Debug, Clone, Eq, PartialEq)]
enum VersionSite {
    /// The member manifest's own `[package].version` (a literal version).
    OwnPackage,
    /// `[workspace.package].version` of the owning workspace-root manifest at
    /// this path (an inherited `version.workspace = true`). The path equals the
    /// member manifest when the root is both a workspace and a package.
    Workspace(PathBuf),
}

/// Decide where a member's version bump lands: its own `[package].version`
/// (literal) or the `[workspace.package].version` of the owning workspace root
/// (`version.workspace = true`).
///
/// This mirrors [`read_declared_version`]'s classification exactly, so the
/// mutation always targets the same manifest the reader resolves the version
/// from. A `[package]` with no `version` key falls back to writing an own
/// literal (a package cargo would otherwise reject).
fn resolve_version_site(doc: &DocumentMut, path: &Path, root: &Path) -> AppResult<VersionSite> {
    let version_item = doc
        .get("package")
        .and_then(Item::as_table_like)
        .and_then(|package| package.get("version"));

    match version_item {
        Some(item) if item.as_str().is_some() => Ok(VersionSite::OwnPackage),
        Some(item) if is_workspace_inherited(item) => {
            let (owner_path, _) = resolve_workspace_root_manifest(doc, path, root)?;
            Ok(VersionSite::Workspace(owner_path))
        }
        Some(_) => Err(AppError::new(
            ErrorCode::InvalidFormat,
            format!(
                "manifest '{}' has a [package].version that is neither a string nor \
                 `version.workspace = true`",
                path.display()
            ),
        )),
        None => Ok(VersionSite::OwnPackage),
    }
}

/// Locate the manifest whose `[workspace.package]` backs a member's
/// `version.workspace = true`, returning its path and parsed document.
///
/// The version may live inline in the member manifest (a root that is both a
/// workspace and a package) or in an ancestor `Cargo.toml`. The ancestor search
/// is bounded to `root` (the working/repository-root trust boundary): a
/// `Cargo.toml` above `root` is never consulted, so resolution cannot reach
/// outside the repository and stays deterministic.
fn resolve_workspace_root_manifest(
    doc: &DocumentMut,
    path: &Path,
    root: &Path,
) -> AppResult<(PathBuf, DocumentMut)> {
    if workspace_package_version(doc).is_some() {
        return Ok((path.to_path_buf(), doc.clone()));
    }

    let mut ancestor = path.parent().and_then(Path::parent);
    while let Some(dir) = ancestor {
        if !dir.starts_with(root) {
            break;
        }
        let candidate = dir.join("Cargo.toml");
        if candidate != path && exists(&candidate)? {
            let text = read_string_bounded(&candidate, MAX_MANIFEST_BYTES)?;
            let manifest = parse_manifest(&text, &candidate)?;
            if manifest.get("workspace").is_some() {
                if workspace_package_version(&manifest).is_some() {
                    return Ok((candidate, manifest));
                }
                return Err(AppError::new(
                    ErrorCode::InvalidFormat,
                    format!(
                        "workspace root '{}' has no [workspace.package].version to inherit",
                        candidate.display()
                    ),
                ));
            }
        }
        ancestor = dir.parent();
    }

    Err(AppError::new(
        ErrorCode::InvalidFormat,
        format!(
            "manifest '{}' declares version.workspace = true but no ancestor workspace root \
             with [workspace.package].version was found",
            path.display()
        ),
    ))
}

/// Read `[workspace.package].version` as a string, if present.
fn workspace_package_version(doc: &DocumentMut) -> Option<&str> {
    doc.get("workspace")
        .and_then(Item::as_table_like)
        .and_then(|workspace| workspace.get("package"))
        .and_then(Item::as_table_like)
        .and_then(|package| package.get("version"))
        .and_then(Item::as_str)
}

/// Parse a semantic version string, attributing failures to `path`.
fn parse_version(raw: &str, path: &Path) -> AppResult<Version> {
    Version::parse(raw).map_err(|error| {
        AppError::new(
            ErrorCode::InvalidFormat,
            format!("invalid version '{raw}' in '{}'", path.display()),
        )
        .with_cause(error)
    })
}

/// Set a member's own `[package].version` to a literal string.
fn set_package_version(doc: &mut DocumentMut, version: &Version, path: &Path) -> AppResult<()> {
    let package = doc
        .get_mut("package")
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| {
            AppError::new(
                ErrorCode::InvalidFormat,
                format!("manifest '{}' has no [package] table", path.display()),
            )
        })?;
    package.insert("version", value(version.to_string()));
    Ok(())
}

/// Set `[workspace.package].version` to a literal string.
fn set_workspace_package_version(
    doc: &mut DocumentMut,
    version: &Version,
    path: &Path,
) -> AppResult<()> {
    let workspace = doc
        .get_mut("workspace")
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| {
            AppError::new(
                ErrorCode::InvalidFormat,
                format!(
                    "workspace root '{}' has no [workspace] table",
                    path.display()
                ),
            )
        })?;
    let package = workspace
        .get_mut("package")
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| {
            AppError::new(
                ErrorCode::InvalidFormat,
                format!(
                    "workspace root '{}' has no [workspace.package] table to bump",
                    path.display()
                ),
            )
        })?;
    package.insert("version", value(version.to_string()));
    Ok(())
}

/// Rewrite a workspace-root manifest's `[workspace.package].version`, returning
/// the format-preserving rewritten body. Used when a version bump must land on
/// a manifest *separate* from the member being released.
fn apply_workspace_root_version(text: &str, path: &Path, version: &Version) -> AppResult<String> {
    let mut doc = parse_manifest(text, path)?;
    set_workspace_package_version(&mut doc, version, path)?;
    Ok(doc.to_string())
}

/// Render a workspace-root manifest's absolute path as a repo-relative
/// [`RepoPath`], failing closed if it escapes the working root.
fn workspace_root_repo_path(owner: &Path, working_root: &Path) -> AppResult<RepoPath> {
    let relative = owner.strip_prefix(working_root).map_err(|_| {
        AppError::invalid_input(
            "release.workspace_root",
            format!(
                "workspace root manifest '{}' is outside the working root '{}'",
                owner.display(),
                working_root.display()
            ),
        )
    })?;
    RepoPath::new(relative)
}

/// Rewrite the version requirement of one intra-project dependency across every
/// dependency table (`[dependencies]`, `[dev-dependencies]`,
/// `[build-dependencies]`).
fn set_dependency_floor(doc: &mut DocumentMut, name: &str, floor: &Version) {
    for table_name in ["dependencies", "dev-dependencies", "build-dependencies"] {
        let Some(table) = doc.get_mut(table_name).and_then(Item::as_table_like_mut) else {
            continue;
        };
        let Some(entry) = table.get_mut(name) else {
            continue;
        };
        match entry {
            Item::Value(existing) if existing.is_str() => {
                *entry = value(floor.to_string());
            }
            _ => {
                if let Some(detail) = entry.as_table_like_mut() {
                    // A workspace-inherited dependency (`{ workspace = true }`) must not gain a
                    // `version` key — cargo rejects the combination.
                    let inherited =
                        detail.get("workspace").and_then(toml_edit::Item::as_bool) == Some(true);
                    if !inherited {
                        detail.insert("version", value(floor.to_string()));
                    }
                }
            }
        }
    }
}

/// Parse a `Cargo.toml` body into an editable document.
fn parse_manifest(text: &str, path: &Path) -> AppResult<DocumentMut> {
    text.parse::<DocumentMut>().map_err(|error| {
        AppError::new(
            ErrorCode::InvalidFormat,
            format!("failed to parse manifest '{}'", path.display()),
        )
        .with_cause(error)
    })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime};

    use rskit_version::semver::Version;

    use toml_edit::Item;
    use toven_ports::ForwardEnvAs;

    use super::{
        apply_workspace_root_version, cargo_token_env_name, create_all, fallback_retry_after,
        parse_cargo_search_versions, parse_manifest, publish_argv, read_declared_version,
        registry_token_injection, remove_stray_sbom_files, retry_after_hint, sbom_argv,
        sbom_output_candidates, set_dependency_floor, set_package_version,
        set_workspace_package_version, version_in_manifest_contents,
    };
    use toven_ports::ReleaseCredentials;

    const MANIFEST: &str = "\
[package]
name = \"app\"
version = \"1.2.3\"

[dependencies]
core = { version = \"0.1.0\", path = \"../core\" }
plain = \"0.4.0\"
";

    /// A root manifest that is both the workspace root and a package, where the
    /// package inherits its version from `[workspace.package]` in the same
    /// file.
    const ROOT_INHERITED_MANIFEST: &str = "\
[workspace]
members = [\".\"]

[workspace.package]
version = \"3.4.5\"

[package]
name = \"app\"
version.workspace = true

[dependencies]
core = { workspace = true }
plain = \"0.4.0\"
";

    #[test]
    fn reads_declared_version() {
        let version =
            read_declared_version(MANIFEST, Path::new("Cargo.toml"), Path::new("")).unwrap();
        assert_eq!(version, Version::new(1, 2, 3));
    }

    #[test]
    fn missing_version_is_rejected() {
        assert!(
            read_declared_version("[package]\nname = \"x\"\n", Path::new("C"), Path::new(""))
                .is_err()
        );
    }

    #[test]
    fn invalid_version_type_is_distinguished_from_missing() {
        // `version` is present but neither a string nor `version.workspace = true`.
        let manifest = "[package]\nname = \"x\"\nversion = 1\n";
        let error = read_declared_version(manifest, Path::new("C"), Path::new("")).unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("neither a string nor"),
            "expected an invalid-type message, got: {message}"
        );
        assert!(
            !message.contains("has no [package].version"),
            "invalid type must not report a missing version: {message}"
        );
    }

    #[test]
    fn resolves_version_inherited_from_workspace_in_same_manifest() {
        let version = read_declared_version(
            ROOT_INHERITED_MANIFEST,
            Path::new("Cargo.toml"),
            Path::new(""),
        )
        .unwrap();
        assert_eq!(version, Version::new(3, 4, 5));
    }

    #[test]
    fn version_in_manifest_reads_an_explicit_package_version() {
        let version = version_in_manifest_contents(MANIFEST).unwrap();
        assert_eq!(version, Some(Version::new(1, 2, 3)));
    }

    #[test]
    fn version_in_manifest_is_none_for_a_workspace_inherited_version() {
        assert_eq!(
            version_in_manifest_contents(ROOT_INHERITED_MANIFEST).unwrap(),
            None
        );
    }

    #[test]
    fn version_in_manifest_is_none_when_no_package_version_is_declared() {
        assert_eq!(
            version_in_manifest_contents("[package]\nname = \"x\"\n").unwrap(),
            None
        );
    }

    #[test]
    fn version_in_manifest_rejects_a_non_string_non_inherited_version() {
        assert!(version_in_manifest_contents("[package]\nname = \"x\"\nversion = 1\n").is_err());
    }

    #[test]
    fn inherited_version_without_a_resolvable_workspace_root_is_rejected() {
        // `version.workspace = true` but no `[workspace.package].version` reachable
        // (the manifest has no inline workspace table and the path has no ancestors).
        let manifest = "[package]\nname = \"x\"\nversion.workspace = true\n";
        assert!(read_declared_version(manifest, Path::new("Cargo.toml"), Path::new("")).is_err());
    }

    #[test]
    fn dep_floor_skips_workspace_inherited_dependencies() {
        // `core` is workspace-inherited; bumping its floor must not stamp a `version`
        // key onto it (cargo forbids `workspace = true` + `version`).
        let mut doc = parse_manifest(ROOT_INHERITED_MANIFEST, Path::new("Cargo.toml")).unwrap();
        set_dependency_floor(&mut doc, "core", &Version::new(0, 2, 0));

        let core = doc["dependencies"]["core"]
            .as_table_like()
            .expect("core is a table");
        assert_eq!(core.get("workspace").and_then(Item::as_bool), Some(true));
        assert!(core.get("version").is_none(), "no version stamped");
    }

    #[test]
    fn applies_own_version_and_dep_floors() {
        let mut doc = parse_manifest(MANIFEST, Path::new("Cargo.toml")).unwrap();
        set_package_version(&mut doc, &Version::new(2, 0, 0), Path::new("Cargo.toml")).unwrap();
        set_dependency_floor(&mut doc, "core", &Version::new(0, 2, 0));
        set_dependency_floor(&mut doc, "plain", &Version::new(0, 5, 0));
        let rewritten = doc.to_string();

        assert!(rewritten.contains("version = \"2.0.0\""));
        assert!(rewritten.contains("core = { version = \"0.2.0\""));
        assert!(rewritten.contains("path = \"../core\""));
        assert!(rewritten.contains("plain = \"0.5.0\""));
    }

    #[test]
    fn unknown_dep_floor_is_ignored() {
        let mut doc = parse_manifest(MANIFEST, Path::new("Cargo.toml")).unwrap();
        set_dependency_floor(&mut doc, "absent", &Version::new(9, 9, 9));
        assert!(!doc.to_string().contains("9.9.9"));
    }

    #[test]
    fn set_workspace_package_version_bumps_the_shared_version_in_place() {
        // A root that is both workspace and package: the bump lands on
        // `[workspace.package].version`, and the member `[package]` keeps its
        // `version.workspace = true` inheritance untouched.
        let mut doc = parse_manifest(ROOT_INHERITED_MANIFEST, Path::new("Cargo.toml")).unwrap();
        set_workspace_package_version(&mut doc, &Version::new(9, 9, 9), Path::new("Cargo.toml"))
            .unwrap();
        let rewritten = doc.to_string();

        assert!(
            rewritten.contains("[workspace.package]\nversion = \"9.9.9\""),
            "workspace version bumped: {rewritten}"
        );
        assert!(
            rewritten.contains("version.workspace = true"),
            "member inheritance preserved: {rewritten}"
        );
        assert!(
            !rewritten.contains("[package]\nname = \"app\"\nversion = \"9.9.9\""),
            "member [package] must not gain a literal version: {rewritten}"
        );
    }

    #[test]
    fn apply_workspace_root_version_rewrites_only_the_workspace_version() {
        // A virtual workspace root (no `[package]`): the bump lands on
        // `[workspace.package].version` alone.
        let root = "\
[workspace]
members = [\"crates/app\"]

[workspace.package]
version = \"0.3.0\"
";
        let rewritten =
            apply_workspace_root_version(root, Path::new("Cargo.toml"), &Version::new(0, 4, 0))
                .unwrap();
        assert!(rewritten.contains("version = \"0.4.0\""));
        assert!(!rewritten.contains("0.3.0"));
    }

    #[test]
    fn apply_workspace_root_version_rejects_a_root_without_a_workspace_package_table() {
        let root = "[workspace]\nmembers = [\"crates/app\"]\n";
        assert!(
            apply_workspace_root_version(root, Path::new("Cargo.toml"), &Version::new(0, 4, 0))
                .is_err()
        );
    }

    #[test]
    fn parses_matching_crate_version_from_cargo_search() {
        // Representative `cargo search <name> --limit 1` stdout: one `name = "ver"`
        // line per crate, optionally trailed by a `# description` comment.
        let stdout = "\
core = \"1.4.2\"    # A core crate
";
        assert_eq!(
            parse_cargo_search_versions("core", stdout),
            vec![Version::new(1, 4, 2)]
        );
    }

    #[test]
    fn retry_after_hint_honors_a_future_crates_io_deadline() {
        // 2100-01-01T00:00:00Z is 4_102_444_800 epoch seconds.
        const DEADLINE_EPOCH_SECS: u64 = 4_102_444_800;
        // The exact rate-limit rejection crates.io emits, lowercased as the
        // classifier sees it. The deadline sits far in the future so the hint
        // must be preferred over any cadence fallback.
        let output = "error: failed to publish to registry at https://crates.io\n\
            caused by: the remote server responded with an error (status 429 too many requests): \
            you have published too many new crates in a short period of time. \
            please try again after fri, 01 jan 2100 00:00:00 gmt and see \
            https://crates.io/docs/rate-limits for more details.";
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let deadline = retry_after_hint(output, now).expect("a future GMT deadline parses");
        assert_eq!(
            deadline,
            SystemTime::UNIX_EPOCH + Duration::from_secs(DEADLINE_EPOCH_SECS)
        );
    }

    #[test]
    fn retry_after_hint_ignores_a_past_deadline() {
        // A deadline already in the past (clock skew, or a stale message) must
        // not be honored — the caller falls back to the cadence instead.
        let output = "status 429 too many requests. please try again after \
            sun, 06 nov 1994 08:49:37 gmt and see https://crates.io/docs/rate-limits.";
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        assert_eq!(retry_after_hint(output, now), None);
    }

    #[test]
    fn retry_after_hint_absent_without_a_deadline_marker() {
        // A 429 with no "try again after" hint yields nothing to schedule from.
        let output = "status 429 too many requests: slow down.";
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        assert_eq!(retry_after_hint(output, now), None);
    }

    #[test]
    fn fallback_cadence_is_longer_for_a_brand_new_crate_name() {
        // crates.io throttles a first-ever crate name (10-minute refill) harder
        // than a new version of an existing name (1-minute refill).
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        assert_eq!(
            fallback_retry_after(true, now),
            Some(now + Duration::from_mins(10))
        );
        assert_eq!(
            fallback_retry_after(false, now),
            Some(now + Duration::from_mins(1))
        );
    }

    #[test]
    fn registry_token_injection_maps_a_configured_var_to_cargo_registry_token() {
        // A configured token_env is forwarded to cargo under CARGO_REGISTRY_TOKEN
        // — the name cargo reads — by name only; the value is never carried back
        // across the seam. The accessor confirms presence (fail-closed) but its
        // value is discarded.
        let credentials = ReleaseCredentials::new(Some("MY_REGISTRY_TOKEN".into()), None);
        let injected = registry_token_injection(&credentials, |name| {
            (name == "MY_REGISTRY_TOKEN").then(|| "s3cr3t".to_string())
        })
        .expect("a present token var resolves");
        assert_eq!(
            injected,
            Some(ForwardEnvAs::new(
                "MY_REGISTRY_TOKEN",
                "CARGO_REGISTRY_TOKEN"
            ))
        );
    }

    #[test]
    fn registry_token_injection_targets_a_named_registry_token_var() {
        // A named alternate registry forwards the same configured source var to
        // cargo under CARGO_REGISTRIES_<NAME>_TOKEN — the name cargo reads for
        // that registry — by name only.
        let credentials = ReleaseCredentials::new(Some("CI_TOKEN".into()), Some("my-corp".into()));
        let injected = registry_token_injection(&credentials, |name| {
            (name == "CI_TOKEN").then(|| "s3cr3t".to_string())
        })
        .expect("a present token var resolves");
        assert_eq!(
            injected,
            Some(ForwardEnvAs::new(
                "CI_TOKEN",
                "CARGO_REGISTRIES_MY_CORP_TOKEN"
            ))
        );
    }

    #[test]
    fn registry_token_injection_is_absent_without_a_configured_var() {
        // No token_env means "use cargo's ambient credential" — inject nothing.
        let injected = registry_token_injection(&ReleaseCredentials::default(), |_| {
            panic!("the accessor must not be consulted when no token_env is configured")
        })
        .expect("no token_env resolves to no injection");
        assert_eq!(injected, None);
    }

    #[test]
    fn registry_token_injection_fails_closed_when_the_named_var_is_absent() {
        // A configured-but-absent credential source must fail closed rather than
        // silently attempt an unauthenticated publish.
        let credentials = ReleaseCredentials::new(Some("MISSING_TOKEN".into()), None);
        let error = registry_token_injection(&credentials, |_| None)
            .expect_err("an unset named token var must be a typed error");
        let message = error.to_string();
        assert!(message.contains("release.token_env"), "{message}");
        assert!(message.contains("MISSING_TOKEN"), "{message}");
    }

    #[test]
    fn publish_argv_targets_crates_io_by_default_and_a_named_alternate_registry() {
        let manifest = Path::new("crates/core/Cargo.toml");
        // The cargo default registry (None or crates.io's canonical names) adds
        // no `--registry` flag: crates.io behavior is unchanged.
        for default in [None, Some("crates-io"), Some("crates.io")] {
            let argv = publish_argv(manifest, default);
            assert_eq!(&argv[0..2], &["publish", "--manifest-path"]);
            assert!(argv.iter().all(|arg| arg != "--registry"), "{default:?}");
        }
        // A named alternate registry routes the publish via `--registry <name>`.
        let argv = publish_argv(manifest, Some("my-corp"));
        let idx = argv
            .iter()
            .position(|arg| arg == "--registry")
            .expect("flag");
        assert_eq!(argv[idx + 1], "my-corp");
    }

    #[test]
    fn cargo_token_env_name_follows_cargos_registry_convention() {
        assert_eq!(cargo_token_env_name(None), "CARGO_REGISTRY_TOKEN");
        assert_eq!(
            cargo_token_env_name(Some("crates-io")),
            "CARGO_REGISTRY_TOKEN"
        );
        assert_eq!(
            cargo_token_env_name(Some("my-corp")),
            "CARGO_REGISTRIES_MY_CORP_TOKEN"
        );
    }

    #[test]
    fn cargo_search_parse_ignores_non_matching_and_malformed_lines() {
        let stdout = "\
core-extra = \"9.9.9\"    # different crate, prefix match must not count
not a versions line
core = \"0.2.0\"
";
        // Only the exact-name match is returned; the prefix-similar crate and the
        // malformed line are skipped.
        assert_eq!(
            parse_cargo_search_versions("core", stdout),
            vec![Version::new(0, 2, 0)]
        );
        assert!(parse_cargo_search_versions("absent", stdout).is_empty());
    }

    #[test]
    fn sbom_argv_is_an_argv_only_cyclonedx_invocation() {
        let manifest = Path::new("/repo/crates/core/Cargo.toml");
        assert_eq!(
            sbom_argv(manifest, "core"),
            vec![
                "cyclonedx".to_string(),
                "--manifest-path".to_string(),
                "/repo/crates/core/Cargo.toml".to_string(),
                "--format".to_string(),
                "json".to_string(),
                "--override-filename".to_string(),
                "core".to_string(),
            ]
        );
    }

    #[test]
    fn sbom_output_candidates_probe_both_suffixes_next_to_the_manifest() {
        let manifest = Path::new("/repo/crates/core/Cargo.toml");
        assert_eq!(
            sbom_output_candidates(manifest, "core"),
            vec![
                PathBuf::from("/repo/crates/core/core.cdx.json"),
                PathBuf::from("/repo/crates/core/core.json"),
            ]
        );
    }

    #[test]
    fn remove_stray_sbom_files_deletes_only_the_stem_copies() {
        use rskit_fs::TempDir;
        use rskit_fs::sync_io::file::exists;

        let root = TempDir::new().expect("temp dir");
        let member_a = root.path().join("crates/a");
        let member_b = root.path().join("crates/b");
        create_all(&member_a).expect("member a");
        create_all(&member_b).expect("member b");

        // cargo cyclonedx scattered `core.json` next to each member; an unrelated
        // committed file with a different stem must survive.
        for dir in [&member_a, &member_b] {
            std::fs::write(dir.join("core.json"), b"{}").expect("stray");
            std::fs::write(dir.join("keep.json"), b"{}").expect("keep");
        }
        std::fs::write(member_a.join("core.cdx.json"), b"{}").expect("stray cdx");

        remove_stray_sbom_files(&[member_a.clone(), member_b.clone()], "core", None)
            .expect("cleanup");

        assert!(!exists(&member_a.join("core.json")).expect("a json"));
        assert!(!exists(&member_a.join("core.cdx.json")).expect("a cdx"));
        assert!(!exists(&member_b.join("core.json")).expect("b json"));
        assert!(exists(&member_a.join("keep.json")).expect("keep a"));
        assert!(exists(&member_b.join("keep.json")).expect("keep b"));
    }

    #[test]
    fn remove_stray_sbom_files_preserves_the_final_artifact() {
        use rskit_fs::TempDir;
        use rskit_fs::sync_io::file::exists;

        let root = TempDir::new().expect("temp dir");
        let member = root.path().join("crates/core");
        let output = member.join("core.cdx.json");
        create_all(&member).expect("member");
        std::fs::write(&output, b"{}").expect("artifact");

        remove_stray_sbom_files(std::slice::from_ref(&member), "core", Some(&output))
            .expect("cleanup");

        assert!(exists(&output).expect("artifact exists"));
    }
}

#[cfg(test)]
mod tag_scheme_tests {
    use std::sync::Arc;

    use rskit_version::semver::Version;
    use toven_model::{EcosystemId, Module, ModuleRef, RepoPath};
    use toven_ports::TagGrammar;
    use toven_testkit::doubles::FakeToolRunner;

    use super::CargoRegistryTarget;

    fn target() -> CargoRegistryTarget {
        CargoRegistryTarget::new(Arc::new(FakeToolRunner::new()))
    }

    fn module() -> Module {
        Module::new(
            ModuleRef::new(EcosystemId::new("rust").expect("ecosystem"), "core").expect("module"),
            RepoPath::new("crates/core").expect("path"),
        )
    }

    #[test]
    fn default_tag_scheme_preserves_existing_rust_tags() {
        let scheme = target().tag_scheme(&module(), None).expect("scheme");

        assert_eq!(scheme.format(&Version::new(1, 2, 3)), "rust/core@1.2.3");
        assert_eq!(scheme.parse("rust/core@1.2.3"), Some(Version::new(1, 2, 3)));
    }

    #[test]
    fn release_defaults_are_registry_and_umbrella_anchored() {
        use toven_ports::{BaselineSourceConfig, ReleaseDefaultsSource, TagMode};

        // crates.io is the registry and one umbrella tag anchors the workspace:
        // the baseline is `max(registry, umbrella-tag)` and both per-crate and
        // umbrella tags are cut.
        let defaults = target().release_defaults();
        assert_eq!(defaults.baseline, BaselineSourceConfig::RegistryUmbrella);
        assert_eq!(defaults.tag_mode, TagMode::Both);
    }

    #[test]
    fn publishing_a_non_public_version_to_crates_io_fails_closed() {
        use toven_ports::{Artifact, Publisher, ReleaseCredentials, Visibility};

        // crates.io publishes every version world-readable, so the adapter is the
        // last line of defense: a private/internal exposure is rejected before
        // any cargo invocation, with a typed, actionable error.
        let artifact = Artifact::new(std::path::PathBuf::from("crates/core"));
        let error = target()
            .publish(
                &module(),
                &artifact,
                &ReleaseCredentials::default(),
                Visibility::Private,
            )
            .expect_err("a private crates.io publish must fail closed");

        assert!(error.to_string().contains("release.visibility"));
        assert!(
            error
                .to_string()
                .contains("crates.io only publishes public")
        );
    }

    #[test]
    fn publishing_a_non_public_version_to_a_named_registry_bypasses_the_crates_io_gate() {
        use toven_ports::{Artifact, Publisher, ReleaseCredentials, Visibility};

        // A named alternate registry is not the public-only crates.io, so the
        // adapter does not reject a private/internal exposure: the publish is
        // allowed to proceed (here it fails later, at manifest resolution — never
        // at the visibility gate).
        let artifact = Artifact::new(std::path::PathBuf::from("crates/core"));
        let error = target()
            .publish(
                &module(),
                &artifact,
                &ReleaseCredentials::new(None, Some("my-corp".into())),
                Visibility::Private,
            )
            .expect_err("no manifest on the fixture module, so publish still fails");

        assert!(
            !error.to_string().contains("release.visibility"),
            "a named registry must not trip the crates.io visibility gate: {error}"
        );
        assert!(error.to_string().contains("manifest"), "{error}");
    }

    #[test]
    fn override_tag_scheme_splits_around_version() {
        let scheme = target()
            .tag_scheme(&module(), Some("{module}/v{version}-release"))
            .expect("scheme");

        assert_eq!(scheme.format(&Version::new(1, 2, 3)), "core/v1.2.3-release");
    }

    #[test]
    fn override_without_version_is_rejected() {
        let error = target()
            .tag_scheme(&module(), Some("{module}"))
            .expect_err("missing version rejected");

        assert!(error.to_string().contains("{version}"));
    }

    #[test]
    fn override_with_channel_is_rejected() {
        // `{channel}` cannot appear in a static tag prefix/suffix — the channel is
        // already part of `{version}` (e.g. `1.0.0-rc.1`), so a `{channel}` in a tag
        // template would always render empty. Reject it instead of silently dropping
        // it.
        let error = target()
            .tag_scheme(&module(), Some("{module}-{channel}/v{version}"))
            .expect_err("channel placeholder rejected");

        assert!(error.to_string().contains("{channel}"));
    }
}
