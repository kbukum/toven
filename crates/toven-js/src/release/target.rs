//! The [`NpmRegistryTarget`] phase implementations and their command runs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::TempDir;
use rskit_fs::sync_io::{dir, file};
use rskit_version::semver::Version;
use toven_model::{Module, RepoPath};
use toven_ports::{
    Artifact, BaselineSourceConfig, ForwardEnvAs, ManifestMutator, Packager, PublishOutcome,
    Publisher, ReleaseCredentials, ReleaseDefaults, ReleaseDefaultsSource, ReleaseMutation,
    ReleaseTemplate, SbomProducer, TagGrammar, TagMode, TagScheme, ToolInvocation, ToolOutcome,
    ToolRunner, VersionSource, Visibility,
};

use crate::config::JsConfig;
use crate::discovery::JS;
use crate::manifest::MAX_MANIFEST_BYTES;
use crate::package_manager::Manager;
use crate::roots::{self, Root};

use super::pack::PackCommand;
use super::{manifest, registry};

/// The default tag: npm's own `name@version` convention.
const DEFAULT_TAG_FORMAT: &str = "{package}@{version}";

/// Bound on a registry query (`npm view`).
const QUERY_TIMEOUT: Duration = Duration::from_mins(1);

/// Bound on packing, which runs the package's `prepack` scripts or the
/// root's configured pack command.
const PACK_TIMEOUT: Duration = Duration::from_mins(10);

/// Bound on one publish attempt.
const PUBLISH_TIMEOUT: Duration = Duration::from_mins(5);

/// Bound on a lockfile refresh, which may resolve against the registry.
const REFRESH_TIMEOUT: Duration = Duration::from_mins(10);

/// Bound on SBOM generation.
const SBOM_TIMEOUT: Duration = Duration::from_mins(5);

/// Retained output for pack, publish, and refresh runs.
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;

/// Retained output for a version list; popular packages have thousands.
const MAX_QUERY_OUTPUT_BYTES: usize = 8 * 1024 * 1024;

/// Retained output for an SBOM, which npm prints to stdout.
const MAX_SBOM_OUTPUT_BYTES: usize = 32 * 1024 * 1024;

/// Bound on a lockfile read when checking whether a refresh changed it.
const MAX_LOCKFILE_BYTES: u64 = 256 * 1024 * 1024;

/// Prefix of the sibling temp file an atomic manifest write uses.
const MANIFEST_TEMP_PREFIX: &str = "toven-package-json";

/// Prefix of the sibling temp file an atomic SBOM write uses.
const SBOM_TEMP_PREFIX: &str = "toven-npm-sbom";

/// Where packed tarballs go: a directory the target creates on first use and
/// deletes on drop, or a caller-owned directory.
enum PackRoot {
    Owned(Mutex<Option<TempDir>>),
    #[cfg(test)]
    Given(PathBuf),
}

/// The npm registry release target for the `js` ecosystem.
///
/// Resolves each module against `project_root` (the engine runs from the
/// repository root).
pub(crate) struct NpmRegistryTarget {
    config: JsConfig,
    runner: Arc<dyn ToolRunner>,
    project_root: PathBuf,
    pack_root: PackRoot,
    roots: Mutex<Option<Arc<Vec<Root>>>>,
}

impl NpmRegistryTarget {
    /// A target over `project_root` that packs into a directory it owns.
    pub(crate) fn new(
        config: JsConfig,
        runner: Arc<dyn ToolRunner>,
        project_root: PathBuf,
    ) -> Self {
        Self {
            config,
            runner,
            project_root,
            pack_root: PackRoot::Owned(Mutex::new(None)),
            roots: Mutex::new(None),
        }
    }

    /// Pack into the caller-owned directory `pack_root` instead.
    #[cfg(test)]
    pub(super) fn with_pack_root(mut self, pack_root: PathBuf) -> Self {
        self.pack_root = PackRoot::Given(pack_root);
        self
    }

    /// The JavaScript roots, resolved once per target.
    fn roots(&self) -> AppResult<Arc<Vec<Root>>> {
        let mut cached = self.roots.lock().map_err(|_| poisoned("root cache"))?;
        if let Some(roots) = cached.as_ref() {
            return Ok(Arc::clone(roots));
        }
        let roots = Arc::new(roots::resolve(&self.config, &self.project_root)?);
        *cached = Some(Arc::clone(&roots));
        drop(cached);
        Ok(roots)
    }

    /// The repo-relative directory and package manager of the root that owns
    /// `module`, from the workspace discovery recorded on it.
    fn root_of(&self, module: &Module) -> AppResult<(PathBuf, Manager)> {
        let dir = module
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.as_str().strip_prefix(JS))
            .and_then(|rest| {
                if rest.is_empty() {
                    Some(PathBuf::new())
                } else {
                    rest.strip_prefix(':').map(PathBuf::from)
                }
            })
            .ok_or_else(|| {
                AppError::invalid_input(
                    "module.workspace",
                    format!("module '{}' belongs to no JavaScript root", module.id),
                )
            })?;
        self.roots()?
            .iter()
            .find(|root| root.dir == dir)
            .map(|root| (root.dir.clone(), root.manager))
            .ok_or_else(|| {
                AppError::invalid_input(
                    "module.workspace",
                    format!(
                        "module '{}' names JavaScript root '{}', which no longer exists",
                        module.id,
                        roots::label(&dir)
                    ),
                )
            })
    }

    /// The module's `package.json`: its repo-relative label and absolute path.
    fn manifest_path(&self, module: &Module) -> AppResult<(String, PathBuf)> {
        let manifest = module.manifest.as_ref().ok_or_else(|| {
            AppError::invalid_input(
                "module.manifest",
                format!("module '{}' has no manifest to release", module.id),
            )
        })?;
        let label = manifest.as_path().display().to_string();
        let path =
            rskit_fs::safe_join(&self.project_root, manifest.as_path()).map_err(|error| {
                AppError::invalid_input(
                    "module.manifest",
                    format!("manifest '{label}' escapes the project root: {error}"),
                )
            })?;
        if !file::exists(&path)? {
            return Err(AppError::invalid_input(
                "module.manifest",
                format!(
                    "manifest '{label}' does not exist for module '{}'",
                    module.id
                ),
            ));
        }
        Ok((label, path))
    }

    /// The directory packed tarballs go under.
    fn pack_root(&self) -> AppResult<PathBuf> {
        match &self.pack_root {
            #[cfg(test)]
            PackRoot::Given(path) => Ok(path.clone()),
            PackRoot::Owned(owned) => {
                let mut owned = owned.lock().map_err(|_| poisoned("pack directory"))?;
                let temp = match owned.take() {
                    Some(temp) => temp,
                    None => TempDir::new()?,
                };
                let path = temp.path().to_path_buf();
                *owned = Some(temp);
                drop(owned);
                Ok(path)
            }
        }
    }

    /// Run `argv` in the repo-relative `dir`, bounded and timed out.
    fn run(
        &self,
        argv: Vec<String>,
        dir: &Path,
        timeout: Duration,
        max_output_bytes: usize,
        secret: Option<ForwardEnvAs>,
    ) -> AppResult<ToolOutcome> {
        let mut invocation = ToolInvocation::new(argv)
            .with_working_dir(self.project_root.join(dir))
            .with_timeout(timeout)
            .with_max_output_bytes(max_output_bytes);
        if let Some(secret) = secret {
            invocation = invocation.with_forward_env_as(vec![secret]);
        }
        self.runner.run(&invocation)
    }

    /// Refresh the lockfile of the root at `dir`, returning its repo-relative
    /// path when the refresh changed it. A root without a lockfile has
    /// nothing to refresh.
    fn refresh_lockfile(&self, dir: &Path, manager: Manager) -> AppResult<Option<RepoPath>> {
        let Some(command) = manager.lockfile_refresh() else {
            return Ok(None);
        };
        let mut lockfile = None;
        for name in manager.tool().lockfiles() {
            let path = self.project_root.join(dir).join(name);
            if file::exists(&path)? {
                lockfile = Some((name, path));
                break;
            }
        }
        let Some((name, path)) = lockfile else {
            return Ok(None);
        };
        let before = file::read_bounded(&path, MAX_LOCKFILE_BYTES)?;
        let argv = command.iter().map(ToString::to_string).collect();
        self.run(argv, dir, REFRESH_TIMEOUT, MAX_OUTPUT_BYTES, None)?
            .require_success(&format!(
                "lockfile refresh tool `{}`",
                manager.tool().name()
            ))?;
        if file::read_bounded(&path, MAX_LOCKFILE_BYTES)? == before {
            return Ok(None);
        }
        RepoPath::new(dir.join(name)).map(Some)
    }
}

impl VersionSource for NpmRegistryTarget {
    fn declared_version(&self, module: &Module) -> AppResult<Option<Version>> {
        let (label, path) = self.manifest_path(module)?;
        manifest::version_in(
            &file::read_string_bounded(&path, MAX_MANIFEST_BYTES)?,
            &label,
        )
    }

    /// The versions npm's configured registry reports for the package.
    ///
    /// The query uses the npm configuration of the package's root (`.npmrc`
    /// scoped registries included), not `release.registry`; the publish
    /// classification is the idempotency backstop either way.
    fn published_versions(&self, module: &Module) -> AppResult<Vec<Version>> {
        let Some(package) = module.package.as_deref() else {
            return Ok(Vec::new());
        };
        let (dir, _) = self.root_of(module)?;
        let argv = ["npm", "view", package, "versions", "--json"]
            .map(ToString::to_string)
            .to_vec();
        let output = self.run(argv, &dir, QUERY_TIMEOUT, MAX_QUERY_OUTPUT_BYTES, None)?;
        registry::published_versions(package, &output)
    }

    fn version_in_manifest(&self, manifest: &str) -> AppResult<Option<Version>> {
        manifest::version_in(manifest, "package.json")
    }
}

impl TagGrammar for NpmRegistryTarget {
    fn tag_scheme(&self, module: &Module, tag_format: Option<&str>) -> AppResult<TagScheme> {
        ReleaseTemplate::parse(
            "release.tag_format",
            tag_format.unwrap_or(DEFAULT_TAG_FORMAT),
        )?
        .tag_scheme(module)
    }
}

impl ReleaseDefaultsSource for NpmRegistryTarget {
    fn release_defaults(&self) -> ReleaseDefaults {
        // The registry is the source of truth and each package carries its own
        // version, so change detection anchors on max(registry, own tag) and
        // every package gets its own `name@version` tag.
        ReleaseDefaults::new(BaselineSourceConfig::Registry, TagMode::PerModule)
    }
}

impl ManifestMutator for NpmRegistryTarget {
    fn apply_release(
        &self,
        module: &Module,
        mutation: &ReleaseMutation,
    ) -> AppResult<Vec<RepoPath>> {
        let (label, path) = self.manifest_path(module)?;
        let text = file::read_string_bounded(&path, MAX_MANIFEST_BYTES)?;
        let Some(rewritten) = manifest::apply(
            &text,
            &label,
            mutation.new_version.as_ref(),
            &mutation.dep_floor_import_updates,
        )?
        else {
            return Ok(Vec::new());
        };
        file::write_atomic_replace(&path, rewritten.as_bytes(), MANIFEST_TEMP_PREFIX)?;
        RepoPath::new(label).map(|path| vec![path])
    }

    fn finalize_release(&self, modules: &[&Module]) -> AppResult<Vec<RepoPath>> {
        let mut roots: BTreeMap<PathBuf, Manager> = BTreeMap::new();
        for module in modules {
            let (dir, manager) = self.root_of(module)?;
            roots.insert(dir, manager);
        }
        let mut written = Vec::new();
        for (dir, manager) in roots {
            written.extend(self.refresh_lockfile(&dir, manager)?);
        }
        Ok(written)
    }
}

impl Packager for NpmRegistryTarget {
    fn package(&self, module: &Module) -> AppResult<Artifact> {
        let package = package_name(module)?;
        let version = self.declared_version_required(module)?;
        // Fail before anything is committed when the version has no dist-tag.
        registry::dist_tag(package, &version)?;
        let (dir, manager) = self.root_of(module)?;
        let destination = self
            .pack_root()?
            .join(format!("{}-{version}", module.id.name));
        dir::remove_all_if_exists(&destination)?;
        dir::create_all(&destination)?;
        let root_label = roots::label(&dir);
        let (argv, working_dir, tool) = match self.config.pack.get(&root_label) {
            Some(authored) => {
                let command =
                    PackCommand::parse(&format!("ecosystems.js.pack.{root_label}"), authored)?;
                let module_root = module.root.as_path().display().to_string();
                let argv = command.render(&module_root, &destination)?;
                (argv, PathBuf::new(), command.program().to_string())
            }
            None => (
                manager.pack(&destination),
                module_dir(module),
                manager.tool().name().to_string(),
            ),
        };
        self.run(argv, &working_dir, PACK_TIMEOUT, MAX_OUTPUT_BYTES, None)?
            .require_success(&format!("pack tool `{tool}`"))?;
        let tarballs: Vec<PathBuf> = dir::list(&destination)?
            .into_iter()
            .filter(|entry| entry.is_file && entry.path.extension().is_some_and(|ext| ext == "tgz"))
            .map(|entry| entry.path)
            .collect();
        match tarballs.as_slice() {
            [tarball] => Ok(Artifact::new(tarball)),
            _ => Err(AppError::new(
                ErrorCode::ExternalService,
                format!(
                    "`{tool}` packed {} tarballs for '{package}', expected exactly one",
                    tarballs.len()
                ),
            )),
        }
    }
}

impl Publisher for NpmRegistryTarget {
    fn publish(
        &self,
        module: &Module,
        artifact: &Artifact,
        credentials: &ReleaseCredentials,
        visibility: Visibility,
    ) -> AppResult<PublishOutcome> {
        let package = package_name(module)?;
        let url = registry::registry_url(credentials.registry().unwrap_or(registry::NPMJS))?;
        let access = registry::access(package, visibility)?;
        let tag = registry::dist_tag(package, &self.declared_version_required(module)?)?;
        let secret = token_injection(credentials, &url, rskit_util::env::get_non_empty)?;
        let (dir, _) = self.root_of(module)?;
        let output = self.run(
            publish_argv(&artifact.path, &url, &tag, access),
            &dir,
            PUBLISH_TIMEOUT,
            MAX_OUTPUT_BYTES,
            secret,
        )?;
        registry::classify_publish(package, &output)
    }

    fn validate_publication(
        &self,
        module: &Module,
        registry: &str,
        visibility: Visibility,
    ) -> AppResult<()> {
        let package = package_name(module)?;
        registry::registry_url(registry)?;
        registry::access(package, visibility)?;
        Ok(())
    }
}

impl SbomProducer for NpmRegistryTarget {
    /// A `CycloneDX` SBOM of the package's runtime dependencies, from npm.
    /// Other package managers have no built-in `CycloneDX` export, so their
    /// roots produce none here.
    fn sbom(&self, module: &Module, out_dir: &Path) -> AppResult<Option<Artifact>> {
        let (dir, manager) = self.root_of(module)?;
        if manager != Manager::Npm {
            return Ok(None);
        }
        let mut argv = ["npm", "sbom", "--sbom-format", "cyclonedx", "--omit", "dev"]
            .map(ToString::to_string)
            .to_vec();
        let member = module_dir(module);
        if member != dir {
            let relative = member.strip_prefix(&dir).unwrap_or(&member);
            argv.extend(["--workspace".to_string(), roots::label(relative)]);
        }
        let output = self.run(argv, &dir, SBOM_TIMEOUT, MAX_SBOM_OUTPUT_BYTES, None)?;
        output.require_success("sbom tool `npm`")?;
        dir::create_all(out_dir)?;
        let artifact = out_dir.join(format!("{}.cdx.json", module.id.name));
        file::write_atomic_replace(&artifact, output.stdout.as_bytes(), SBOM_TEMP_PREFIX)?;
        Ok(Some(Artifact::new(artifact)))
    }
}

/// The module's repo-relative directory (empty for the project root).
fn module_dir(module: &Module) -> PathBuf {
    let root = module.root.as_path();
    if root == Path::new(".") {
        PathBuf::new()
    } else {
        root.to_path_buf()
    }
}

/// The package's registry name; a package without one cannot publish.
fn package_name(module: &Module) -> AppResult<&str> {
    module.package.as_deref().ok_or_else(|| {
        AppError::invalid_input(
            "release.registry",
            format!(
                "module '{}' has no \"name\" in its package.json, so it cannot be published; \
                 name it or set publish = false",
                module.id
            ),
        )
    })
}

/// The `npm publish` argv for a packed tarball. Provenance follows npm's own
/// configuration: trusted publishing attaches it automatically, and
/// `NPM_CONFIG_PROVENANCE=true` requests it for token publishes from CI.
pub(super) fn publish_argv(tarball: &Path, url: &str, tag: &str, access: &str) -> Vec<String> {
    vec![
        "npm".to_string(),
        "publish".to_string(),
        tarball.display().to_string(),
        "--registry".to_string(),
        url.to_string(),
        "--tag".to_string(),
        tag.to_string(),
        "--access".to_string(),
        access.to_string(),
    ]
}

/// The mapping that forwards the configured `token_env` to npm as the
/// registry's auth-token config variable, or `None` to let npm use its
/// ambient credential (an `.npmrc` token or trusted publishing).
///
/// `env` is consulted only to fail closed when the named variable is unset or
/// empty; the value is discarded, so the secret never crosses the seam.
pub(super) fn token_injection<F>(
    credentials: &ReleaseCredentials,
    url: &str,
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
    Ok(Some(ForwardEnvAs::new(name, registry::token_env_name(url))))
}

/// The error for a poisoned lock.
fn poisoned(what: &str) -> AppError {
    AppError::new(
        ErrorCode::Internal,
        format!("the npm target's {what} lock was poisoned"),
    )
}
