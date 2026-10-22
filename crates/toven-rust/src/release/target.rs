//! [`CargoRegistryTarget`]: the cargo runners it shares with the manifest,
//! publish and SBOM files, plus its version, tag and packaging contracts.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::safe_join;
use rskit_fs::sync_io::file::{exists, read_string_bounded};
use rskit_version::semver::Version;
use toven_model::Module;
use toven_ports::{
    Artifact, BaselineSourceConfig, ForwardEnvAs, Packager, ReleaseDefaults, ReleaseDefaultsSource,
    ReleaseTemplate, TagGrammar, TagMode, TagScheme, ToolInvocation, ToolOutcome, ToolRunner,
    VersionSource,
};

use super::manifest::{MAX_MANIFEST_BYTES, read_declared_version, version_in_manifest_contents};
use super::publish::parse_cargo_search_versions;

/// Default Rust release tag template.
const DEFAULT_TAG_FORMAT: &str = "{ecosystem}/{module}@{version}";

/// Maximum retained stdout/stderr for cargo registry commands (64 KiB each).
pub(super) const MAX_CARGO_OUTPUT_BYTES: usize = 64 * 1024;

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
    /// The version each workspace-root manifest has been asked to bump to,
    /// keyed by absolute manifest path. A single `release apply` fans
    /// `apply_release` across every member; members of one inheriting workspace
    /// all route their version bump to the same root. Recording the intended
    /// version per root lets a second member that requests a **different**
    /// version (e.g. a semver cascade that bumps one sibling minor and another
    /// patch) fail closed with a typed conflict instead of last-writer-wins
    /// clobbering the shared root while earlier tags used the other version.
    pub(super) workspace_root_versions: Arc<Mutex<BTreeMap<PathBuf, Version>>>,
}

impl CargoRegistryTarget {
    /// Construct the crates.io target.
    #[must_use]
    pub fn new(runner: Arc<dyn ToolRunner>) -> Self {
        Self {
            runner,
            workspace_root_versions: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// Resolve a module's repo-relative manifest to an absolute path under the
    /// current working directory.
    pub(super) fn manifest_path(module: &Module) -> AppResult<PathBuf> {
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
        Ok(self
            .cargo_metadata(manifest)?
            .target_directory
            .into_std_path_buf())
    }

    /// Run and decode `cargo metadata --no-deps` for `manifest`.
    pub(super) fn cargo_metadata(&self, manifest: &Path) -> AppResult<cargo_metadata::Metadata> {
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
        Ok(metadata)
    }

    /// The trust boundary for manifest resolution: the process working
    /// directory (the engine runs from the repository root).
    pub(super) fn working_root() -> AppResult<PathBuf> {
        std::env::current_dir().map_err(|error| {
            AppError::new(ErrorCode::Internal, "failed to read current directory").with_cause(error)
        })
    }

    pub(super) fn cargo<I>(&self, working_dir: PathBuf, args: I) -> AppResult<ToolOutcome>
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
    pub(super) fn cargo_with_env<I>(
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
        ReleaseTemplate::parse(
            "release.tag_format",
            tag_format.unwrap_or(DEFAULT_TAG_FORMAT),
        )?
        .tag_scheme(module)
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

pub(super) fn package_name(module: &Module) -> String {
    module
        .package
        .clone()
        .unwrap_or_else(|| module.id.name.clone())
}

#[cfg(test)]
mod tests {
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
