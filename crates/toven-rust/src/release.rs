//! `CratesIoTarget` — the cargo/crates.io [`ReleaseTarget`] sliver.
//!
//! Owns the ecosystem-specific ~10% of release: reading a module's declared
//! version from its `Cargo.toml`, applying one atomic version mutation (own
//! version + intra-project dependency-floor rewrites) with format-preserving
//! `toml_edit`, and the registry-facing trio (`published_versions`, `package`,
//! `publish`) routed through `cargo` via `rskit-process` with bounded output and
//! a hard timeout. Each method returns typed data or a typed error — never a
//! success-shaped fallback.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::safe_join;
use rskit_fs::sync_io::file::{exists, read_string_bounded, write_atomic_replace};
use rskit_process::{
    CapturedIo, OutputPolicy, ProcessConfig, ProcessIo, ProcessResult, ProcessSpec, run,
};
use rskit_version::semver::Version;
use toml_edit::{DocumentMut, Item, value};
use toven_model::Module;
use toven_ports::{Artifact, PublishOutcome, RegistryCadence, ReleaseMutation, ReleaseTarget};

/// Hard bound on a `Cargo.toml` read (4 MiB) — manifests are tiny; this only
/// guards against a pathological file.
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;

/// Temp-file prefix for atomic manifest rewrites.
const MANIFEST_TEMP_PREFIX: &str = "toven-cargo-manifest";

/// Maximum retained stdout/stderr for cargo registry commands (64 KiB each).
const MAX_CARGO_OUTPUT_BYTES: usize = 64 * 1024;

/// Maximum retained `cargo metadata` output (16 MiB) — large enough for big
/// workspaces, bounded so a runaway process cannot exhaust memory.
const MAX_METADATA_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

/// Timeout for a single cargo registry-facing command.
const CARGO_COMMAND_TIMEOUT: Duration = Duration::from_mins(2);

/// The crates.io release target for the Rust ecosystem.
///
/// Resolves each module's repo-relative `manifest` against the process working
/// directory (the engine runs from the repository root).
#[derive(Debug, Default, Clone, Copy)]
pub struct CratesIoTarget;

impl CratesIoTarget {
    /// Construct the crates.io target.
    #[must_use]
    pub const fn new() -> Self {
        Self
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
        // `module.manifest` is untrusted input from discovery/config: fail fast
        // with a typed input error rather than spawning cargo against a path that
        // does not exist on disk.
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
    fn target_directory(manifest: &Path) -> AppResult<PathBuf> {
        let output = cargo_metadata_command(Self::working_root()?, manifest)?;
        output.check()?;
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

    /// The trust boundary for manifest resolution: the process working directory
    /// (the engine runs from the repository root).
    fn working_root() -> AppResult<PathBuf> {
        std::env::current_dir().map_err(|error| {
            AppError::new(ErrorCode::Internal, "failed to read current directory").with_cause(error)
        })
    }
}

impl ReleaseTarget for CratesIoTarget {
    fn declared_version(&self, module: &Module) -> AppResult<Version> {
        let root = Self::working_root()?;
        let path = Self::manifest_path(module)?;
        let text = read_string_bounded(&path, MAX_MANIFEST_BYTES)?;
        read_declared_version(&text, &path, &root)
    }

    fn published_versions(&self, module: &Module) -> AppResult<Vec<Version>> {
        let package = package_name(module);
        // `cargo search` reports only the single latest version of a crate, so
        // this is the best-effort "latest published" set the port contract
        // allows — the publish loop's `AlreadyPublished` classification is the
        // authoritative idempotency backstop for older versions.
        let output = cargo(
            Self::working_root()?,
            [
                "search".to_string(),
                package.clone(),
                "--limit".to_string(),
                "1".to_string(),
            ],
        )?;
        output.check()?;
        Ok(parse_cargo_search_versions(&package, &output.stdout))
    }

    fn package(&self, module: &Module) -> AppResult<Artifact> {
        let path = Self::manifest_path(module)?;
        let output = cargo(
            Self::working_root()?,
            [
                "package".to_string(),
                "--manifest-path".to_string(),
                path.display().to_string(),
                "--allow-dirty".to_string(),
            ],
        )?;
        output.check()?;

        let artifact = Self::target_directory(&path)?.join("package").join(format!(
            "{}-{}.crate",
            package_name(module),
            self.declared_version(module)?
        ));
        Ok(Artifact::new(artifact))
    }

    fn apply_release(&self, module: &Module, mutation: &ReleaseMutation) -> AppResult<()> {
        let path = Self::manifest_path(module)?;
        let text = read_string_bounded(&path, MAX_MANIFEST_BYTES)?;
        let rewritten = apply_mutation(&text, mutation, &path)?;
        write_atomic_replace(&path, rewritten.as_bytes(), MANIFEST_TEMP_PREFIX)
    }

    fn publish(&self, module: &Module, _artifact: &Artifact) -> AppResult<PublishOutcome> {
        let path = Self::manifest_path(module)?;
        let output = cargo(
            Self::working_root()?,
            [
                "publish".to_string(),
                "--manifest-path".to_string(),
                path.display().to_string(),
                "--allow-dirty".to_string(),
            ],
        )?;
        classify_publish(*self, module, &output)
    }
}

fn cargo<I>(working_dir: PathBuf, args: I) -> AppResult<ProcessResult>
where
    I: IntoIterator<Item = String>,
{
    let spec = ProcessSpec::new("cargo").args(args).dir(working_dir);
    let config = ProcessConfig::default()
        .with_timeout(Some(CARGO_COMMAND_TIMEOUT))
        .with_io(ProcessIo::captured(CapturedIo::new().with_output(
            OutputPolicy::captured().with_max_output_bytes(MAX_CARGO_OUTPUT_BYTES),
        )));
    run(&spec, &config)
}

/// Run `cargo metadata --no-deps` for `manifest`, bounded and timed-out, to read
/// the effective target directory. `metadata` output can be large for big
/// workspaces, so this uses a wider output bound than the registry commands.
fn cargo_metadata_command(working_dir: PathBuf, manifest: &Path) -> AppResult<ProcessResult> {
    let spec = ProcessSpec::new("cargo")
        .args([
            "metadata".to_string(),
            "--no-deps".to_string(),
            "--format-version".to_string(),
            "1".to_string(),
            "--manifest-path".to_string(),
            manifest.display().to_string(),
        ])
        .dir(working_dir);
    let config = ProcessConfig::default()
        .with_timeout(Some(CARGO_COMMAND_TIMEOUT))
        .with_io(ProcessIo::captured(CapturedIo::new().with_output(
            OutputPolicy::captured().with_max_output_bytes(MAX_METADATA_OUTPUT_BYTES),
        )));
    run(&spec, &config)
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
    target: CratesIoTarget,
    module: &Module,
    output: &ProcessResult,
) -> AppResult<PublishOutcome> {
    if output.success() {
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
        // crates.io applies a stricter cadence to a brand-new crate name than to a
        // new version of an existing one. Treat "no versions on the registry" as a
        // first publish; a failed lookup falls back to the existing-name cadence.
        let is_new_release = target
            .published_versions(module)
            .is_ok_and(|versions| versions.is_empty());
        return Ok(PublishOutcome::RateLimited {
            retry_after: fallback_retry_after(is_new_release, SystemTime::now()),
        });
    }

    output.check().map(|_| PublishOutcome::Published)
}

fn fallback_retry_after(is_new_release: bool, now: SystemTime) -> Option<SystemTime> {
    RegistryCadence::new(Duration::from_mins(10), Duration::from_mins(1))
        .fallback_retry_after(is_new_release, now)
}

/// Read `[package].version` from a `Cargo.toml` body.
///
/// A string `version` is returned directly. A workspace-inherited version
/// (`version.workspace = true`) is resolved from the nearest `[workspace.package]
/// version` — first in the same manifest (root package), then by walking ancestor
/// directories from `path` for the workspace-root `Cargo.toml`. The ancestor walk
/// never crosses above `root` (the working/repository-root trust boundary).
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
/// The ancestor search is bounded to `root` (the working/repository-root trust
/// boundary): a `Cargo.toml` above `root` is never consulted, so resolution
/// cannot reach outside the repository and stays deterministic.
fn resolve_inherited_version(doc: &DocumentMut, path: &Path, root: &Path) -> AppResult<Version> {
    if let Some(raw) = workspace_package_version(doc) {
        return parse_version(raw, path);
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
                return workspace_package_version(&manifest)
                    .ok_or_else(|| {
                        AppError::new(
                            ErrorCode::InvalidFormat,
                            format!(
                                "workspace root '{}' has no [workspace.package].version to inherit",
                                candidate.display()
                            ),
                        )
                    })
                    .and_then(|raw| parse_version(raw, &candidate));
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

/// Apply one [`ReleaseMutation`] to a `Cargo.toml` body, returning the rewritten
/// text. Own version and each dependency floor are set with format-preserving
/// edits; the document is otherwise untouched.
fn apply_mutation(text: &str, mutation: &ReleaseMutation, path: &Path) -> AppResult<String> {
    let mut doc = parse_manifest(text, path)?;

    if let Some(new_version) = &mutation.new_version {
        let package = doc
            .get_mut("package")
            .and_then(Item::as_table_like_mut)
            .ok_or_else(|| {
                AppError::new(
                    ErrorCode::InvalidFormat,
                    format!("manifest '{}' has no [package] table", path.display()),
                )
            })?;
        package.insert("version", value(new_version.to_string()));
    }

    for (dependency, floor) in &mutation.dep_floor_updates {
        set_dependency_floor(&mut doc, &dependency.name, floor);
    }

    Ok(doc.to_string())
}

/// Rewrite the version requirement of one intra-project dependency across every
/// dependency table (`[dependencies]`, `[dev-dependencies]`, `[build-dependencies]`).
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
                    // A workspace-inherited dependency (`{ workspace = true }`) must
                    // not gain a `version` key — cargo rejects the combination.
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
    use std::path::Path;

    use rskit_version::semver::Version;
    use toven_model::{EcosystemId, ModuleRef};
    use toven_ports::ReleaseMutation;

    use toml_edit::Item;

    use super::{apply_mutation, parse_cargo_search_versions, read_declared_version};

    const MANIFEST: &str = "\
[package]
name = \"app\"
version = \"1.2.3\"

[dependencies]
core = { version = \"0.1.0\", path = \"../core\" }
plain = \"0.4.0\"
";

    /// A root manifest that is both the workspace root and a package, where the
    /// package inherits its version from `[workspace.package]` in the same file.
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

    fn dep(name: &str) -> ModuleRef {
        ModuleRef::new(EcosystemId::new("rust").unwrap(), name).unwrap()
    }

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
    fn inherited_version_without_a_resolvable_workspace_root_is_rejected() {
        // `version.workspace = true` but no `[workspace.package].version` reachable
        // (the manifest has no inline workspace table and the path has no ancestors).
        let manifest = "[package]\nname = \"x\"\nversion.workspace = true\n";
        assert!(read_declared_version(manifest, Path::new("Cargo.toml"), Path::new("")).is_err());
    }

    #[test]
    fn dep_floor_skips_workspace_inherited_dependencies() {
        let mut mutation = ReleaseMutation::version(Version::new(2, 0, 0));
        // `core` is workspace-inherited; bumping its floor must not stamp a
        // `version` key onto it (cargo forbids `workspace = true` + `version`).
        mutation
            .dep_floor_updates
            .insert(dep("core"), Version::new(0, 2, 0));

        let rewritten =
            apply_mutation(ROOT_INHERITED_MANIFEST, &mutation, Path::new("Cargo.toml")).unwrap();

        let doc = rewritten
            .parse::<toml_edit::DocumentMut>()
            .expect("valid toml");
        let core = doc["dependencies"]["core"]
            .as_table_like()
            .expect("core is a table");
        assert_eq!(core.get("workspace").and_then(Item::as_bool), Some(true));
        assert!(
            core.get("version").is_none(),
            "no version stamped: {rewritten}"
        );
    }

    #[test]
    fn applies_own_version_and_dep_floors() {
        let mut mutation = ReleaseMutation::version(Version::new(2, 0, 0));
        mutation
            .dep_floor_updates
            .insert(dep("core"), Version::new(0, 2, 0));
        mutation
            .dep_floor_updates
            .insert(dep("plain"), Version::new(0, 5, 0));

        let rewritten = apply_mutation(MANIFEST, &mutation, Path::new("Cargo.toml")).unwrap();

        assert!(rewritten.contains("version = \"2.0.0\""));
        assert!(rewritten.contains("core = { version = \"0.2.0\""));
        assert!(rewritten.contains("path = \"../core\""));
        assert!(rewritten.contains("plain = \"0.5.0\""));
    }

    #[test]
    fn unknown_dep_floor_is_ignored() {
        let mut mutation = ReleaseMutation::version(Version::new(2, 0, 0));
        mutation
            .dep_floor_updates
            .insert(dep("absent"), Version::new(9, 9, 9));
        let rewritten = apply_mutation(MANIFEST, &mutation, Path::new("Cargo.toml")).unwrap();
        assert!(!rewritten.contains("9.9.9"));
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
}
