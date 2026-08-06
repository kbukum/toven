//! `release package` verb: turn already-built binaries into the fixed-name
//! archive assets the hosted-release config declares.
//!
//! The engine owns release *policy* — which declared asset a built binary maps
//! to, which archive format that asset name implies, and that the produced
//! bytes are deterministic — while [`rskit_fs::archive`] owns the mechanical
//! packaging. The verb is non-mutating: it never bumps a manifest, tags, or
//! publishes. It consumes one already-built target's binary (cross-target
//! compilation stays a CI matrix concern) and stages exactly the archive asset
//! `[…release.host].assets` declares for that target.
//!
//! Asset → binary mapping is derived, not configured twice: for a target triple
//! `T`, the archive extension is `zip` for a `*windows*` triple and `tar.gz`
//! otherwise, and the one declared asset whose file name ends `-{T}.{ext}` is
//! the output. Its base name (the file name minus that suffix) is the member
//! recorded in the archive — with a `.exe` suffix on Windows — and the binary
//! archived defaults to `target/{T}/release/{member}`. Every failure is fail
//! closed with a typed error: an unknown/traversing target triple, no declared
//! asset for the target, or a missing built binary.

use std::path::{Path, PathBuf};

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::archive::{self, ArchiveEntry};
use rskit_fs::safe_join;
use rskit_fs::sync_io::dir::create_all;
use rskit_fs::sync_io::file::exists as file_exists;
use toven_ports::{Provider, Reporter};

use super::plan::{release_targets, resolve_release_settings};
use toven_engine_core::config::Document;
use toven_engine_core::federation::resolve::PathDriverLocator;
use toven_engine_core::plan::{PlanRequest, prepare_front};

/// Unix mode recorded for a packaged executable (`rwxr-xr-x`).
const EXECUTABLE_MODE: u32 = 0o755;

/// The archive format a declared asset name selects.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub enum ArchiveFormat {
    /// A gzip-compressed tar archive (`.tar.gz`), used for non-Windows targets.
    TarGz,
    /// A zip archive (`.zip`), used for Windows targets.
    Zip,
}

impl ArchiveFormat {
    /// The format implied by a target triple: `Zip` for a `*windows*` triple,
    /// `TarGz` otherwise.
    #[must_use]
    fn for_target(target: &str) -> Self {
        if target.contains("windows") {
            Self::Zip
        } else {
            Self::TarGz
        }
    }

    /// The file-name extension (without a leading dot) for the format.
    #[must_use]
    const fn extension(self) -> &'static str {
        match self {
            Self::TarGz => "tar.gz",
            Self::Zip => "zip",
        }
    }

    /// The canonical format name for typed reporting.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TarGz => "tar.gz",
            Self::Zip => "zip",
        }
    }
}

/// One archive asset the verb produced from a built binary.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PackagedAsset {
    /// The project-relative asset path written (as declared in `host.assets`).
    pub asset: String,
    /// The built binary archived into the asset.
    pub source: PathBuf,
    /// The archive format written.
    pub format: ArchiveFormat,
    /// The produced archive size in bytes.
    pub bytes: u64,
}

/// The typed result of `release package` for one target.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PackageReport {
    /// The target triple that was packaged.
    pub target: String,
    /// The archive assets produced (usually exactly one).
    pub assets: Vec<PackagedAsset>,
}

impl PackageReport {
    /// Construct a package report.
    #[must_use]
    pub const fn new(target: String, assets: Vec<PackagedAsset>) -> Self {
        Self { target, assets }
    }
}

/// Stage the declared archive assets for `target` from already-built binaries.
///
/// `binary_override` supplies the built binary explicitly; when `None`, the
/// binary is located at `target/<target>/release/<member>` under the project
/// root, mirroring `cargo build --release --target <target>`.
///
/// # Errors
/// Fails closed with a typed error when the target triple is malformed, no
/// hosted-release asset is declared for the target, or the built binary is
/// missing — as well as propagating configuration, discovery, graph, archive,
/// and I/O failures.
pub fn release_package(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    target: &str,
    binary_override: Option<&Path>,
    reporter: &mut dyn Reporter,
) -> AppResult<PackageReport> {
    validate_target(target)?;
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

    let declared = super::assets::declared_release_assets(&settings);
    if declared.is_empty() {
        return Err(AppError::invalid_input(
            "release.host.assets",
            "no hosted-release assets are declared; nothing to package (set \
             […release.host] forge + assets)",
        ));
    }

    let format = ArchiveFormat::for_target(target);
    let suffix = format!("-{target}.{}", format.extension());
    let matched: Vec<&String> = declared
        .iter()
        .filter(|asset| asset_file_name(asset).is_some_and(|name| name.ends_with(&suffix)))
        .copied()
        .collect();
    if matched.is_empty() {
        return Err(AppError::invalid_input(
            "release.host.assets",
            format!(
                "no hosted-release asset is declared for target '{target}' (expected an asset \
                 named '*{suffix}')"
            ),
        ));
    }

    let project_root = request.project_root.as_path();
    let mut assets = Vec::with_capacity(matched.len());
    for asset in matched {
        assets.push(package_asset(
            project_root,
            asset,
            target,
            format,
            &suffix,
            binary_override,
        )?);
    }
    assets.sort_by(|left, right| left.asset.cmp(&right.asset));
    Ok(PackageReport::new(target.to_string(), assets))
}

/// Package a single declared asset from its built binary.
fn package_asset(
    project_root: &Path,
    asset: &str,
    target: &str,
    format: ArchiveFormat,
    suffix: &str,
    binary_override: Option<&Path>,
) -> AppResult<PackagedAsset> {
    let file_name = asset_file_name(asset).ok_or_else(|| {
        AppError::invalid_input(
            "release.host.assets",
            format!("asset '{asset}' has no file name to package"),
        )
    })?;
    // The base name (file name minus the `-{target}.{ext}` suffix) is the
    // binary/member name; Windows binaries carry the `.exe` extension.
    let base = file_name.strip_suffix(suffix).ok_or_else(|| {
        AppError::new(
            ErrorCode::Internal,
            format!("asset '{asset}' no longer matches the '{suffix}' suffix"),
        )
    })?;
    let member = match format {
        ArchiveFormat::Zip => format!("{base}.exe"),
        ArchiveFormat::TarGz => base.to_string(),
    };

    let source = binary_override.map_or_else(
        || {
            project_root
                .join("target")
                .join(target)
                .join("release")
                .join(&member)
        },
        Path::to_path_buf,
    );
    if !file_exists(&source)? {
        return Err(AppError::invalid_input(
            "release.package.binary",
            format!(
                "built binary not found at '{}' for target '{target}'; build it before packaging",
                source.display()
            ),
        ));
    }

    let out = safe_join(project_root, asset).map_err(|error| {
        AppError::invalid_input(
            "release.host.assets",
            format!("asset '{asset}' is not a safe project-relative path"),
        )
        .with_cause(error)
    })?;
    if let Some(parent) = out.parent() {
        create_all(parent)?;
    }

    let entries = [ArchiveEntry::new(member, source.clone(), EXECUTABLE_MODE)];
    match format {
        ArchiveFormat::TarGz => archive::tar_gz(&entries, &out)?,
        ArchiveFormat::Zip => archive::zip(&entries, &out)?,
    }

    let bytes = std::fs::metadata(&out)
        .map(|meta| meta.len())
        .map_err(|error| {
            AppError::new(
                ErrorCode::Internal,
                format!("cannot stat produced archive '{}': {error}", out.display()),
            )
            .with_cause(error)
        })?;
    Ok(PackagedAsset {
        asset: asset.to_string(),
        source,
        format,
        bytes,
    })
}

/// The final path component of a project-relative asset path.
fn asset_file_name(asset: &str) -> Option<&str> {
    Path::new(asset).file_name().and_then(|name| name.to_str())
}

/// Reject a target triple that is empty or carries any character outside the
/// triple alphabet, closing the path-traversal hole a crafted `target` would
/// otherwise open when it is joined into `target/<triple>/release`.
fn validate_target(target: &str) -> AppResult<()> {
    if target.is_empty()
        || !target.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
    {
        return Err(AppError::invalid_input(
            "release.package.target",
            format!(
                "invalid target triple '{target}' (expected e.g. \
                 x86_64-unknown-linux-gnu: lowercase letters, digits, '_', '-')"
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use rskit_config::RawValue;
    use rskit_fs::TempDir;
    use serde_json::json;
    use toven_model::{AbsPath, EcosystemId, Module, ModuleRef, RepoPath};
    use toven_ports::{
        CommonEcosystemConfig, DiscoverResponse, HostConfig, Provider, ReleaseConfig, TaskIntent,
    };
    use toven_testkit::{
        FakeConfiguredAdapter, FakeProvider, FakeReleaseTarget, RecordingReporter,
    };

    use super::{ArchiveFormat, release_package};
    use toven_engine_core::config::{Document, ProjectConfig, TovenConfig};
    use toven_engine_core::plan::PlanRequest;

    const LINUX: &str = "x86_64-unknown-linux-gnu";
    const WINDOWS: &str = "x86_64-pc-windows-msvc";

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

    fn request(root: &Path) -> PlanRequest {
        PlanRequest::new(
            "r1",
            "demo",
            TaskIntent::resolve("release"),
            AbsPath::new(root.to_str().unwrap()).unwrap(),
        )
    }

    /// Build a provider whose ecosystem declares `assets` as hosted-release
    /// assets on a `github` forge.
    fn provider_with_assets(assets: Vec<&str>) -> FakeProvider {
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![module("core")];
        let common = CommonEcosystemConfig {
            release: ReleaseConfig {
                host: Some(HostConfig {
                    forge: Some("github".to_string()),
                    assets: Some(assets.into_iter().map(str::to_string).collect()),
                    ..HostConfig::default()
                }),
                ..ReleaseConfig::default()
            },
            ..CommonEcosystemConfig::default()
        };
        let adapter = FakeConfiguredAdapter::new(eid("rust"))
            .with_response(response)
            .with_release_target(FakeReleaseTarget::new())
            .with_common(common);
        FakeProvider::new(eid("rust")).with_adapter(adapter)
    }

    /// Write a fake built binary at `target/<triple>/release/<name>` under
    /// `root`, returning its path.
    fn write_built_binary(root: &Path, triple: &str, name: &str) {
        let dir = root.join("target").join(triple).join("release");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), b"fake-toven-binary").unwrap();
    }

    #[test]
    fn packages_the_declared_tar_gz_from_the_built_binary() {
        let root = TempDir::new().unwrap();
        write_built_binary(root.path(), LINUX, "toven");
        let provider = provider_with_assets(vec!["dist/toven-x86_64-unknown-linux-gnu.tar.gz"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let mut reporter = RecordingReporter::new();

        let report = release_package(
            &request(root.path()),
            &document(),
            &providers,
            LINUX,
            None,
            &mut reporter,
        )
        .unwrap();

        assert_eq!(report.target, LINUX);
        assert_eq!(report.assets.len(), 1);
        let asset = &report.assets[0];
        assert_eq!(asset.asset, "dist/toven-x86_64-unknown-linux-gnu.tar.gz");
        assert_eq!(asset.format, ArchiveFormat::TarGz);
        assert!(asset.bytes > 0);
        let produced = root
            .path()
            .join("dist/toven-x86_64-unknown-linux-gnu.tar.gz");
        assert!(
            produced.is_file(),
            "archive must be written to the asset path"
        );
    }

    #[test]
    fn produced_archive_is_byte_stable() {
        let root = TempDir::new().unwrap();
        write_built_binary(root.path(), LINUX, "toven");
        let provider = provider_with_assets(vec!["dist/toven-x86_64-unknown-linux-gnu.tar.gz"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let mut reporter = RecordingReporter::new();
        let produced = root
            .path()
            .join("dist/toven-x86_64-unknown-linux-gnu.tar.gz");

        release_package(
            &request(root.path()),
            &document(),
            &providers,
            LINUX,
            None,
            &mut reporter,
        )
        .unwrap();
        let first = std::fs::read(&produced).unwrap();
        release_package(
            &request(root.path()),
            &document(),
            &providers,
            LINUX,
            None,
            &mut reporter,
        )
        .unwrap();
        let second = std::fs::read(&produced).unwrap();
        assert_eq!(
            first, second,
            "repackaging identical input must be byte-stable"
        );
    }

    #[test]
    fn packages_a_windows_zip_with_an_exe_member() {
        let root = TempDir::new().unwrap();
        write_built_binary(root.path(), WINDOWS, "toven.exe");
        let provider = provider_with_assets(vec!["dist/toven-x86_64-pc-windows-msvc.zip"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let mut reporter = RecordingReporter::new();

        let report = release_package(
            &request(root.path()),
            &document(),
            &providers,
            WINDOWS,
            None,
            &mut reporter,
        )
        .unwrap();

        assert_eq!(report.assets.len(), 1);
        assert_eq!(report.assets[0].format, ArchiveFormat::Zip);
        let produced = root.path().join("dist/toven-x86_64-pc-windows-msvc.zip");
        let bytes = std::fs::read(&produced).unwrap();
        assert_eq!(&bytes[..2], b"PK", "a zip archive starts with the PK magic");
    }

    #[test]
    fn fails_closed_when_the_built_binary_is_missing() {
        let root = TempDir::new().unwrap();
        // No binary written.
        let provider = provider_with_assets(vec!["dist/toven-x86_64-unknown-linux-gnu.tar.gz"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let mut reporter = RecordingReporter::new();

        let error = release_package(
            &request(root.path()),
            &document(),
            &providers,
            LINUX,
            None,
            &mut reporter,
        )
        .expect_err("a missing built binary must fail closed");
        assert!(
            error.to_string().contains("built binary not found"),
            "{error}"
        );
    }

    #[test]
    fn fails_closed_when_no_asset_is_declared_for_the_target() {
        let root = TempDir::new().unwrap();
        write_built_binary(root.path(), LINUX, "toven");
        // A non-archive asset and a different-target archive: neither matches
        // the requested Linux target.
        let provider = provider_with_assets(vec![
            "dist/toven-sbom.cdx.json",
            "dist/toven-aarch64-apple-darwin.tar.gz",
        ]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let mut reporter = RecordingReporter::new();

        let error = release_package(
            &request(root.path()),
            &document(),
            &providers,
            LINUX,
            None,
            &mut reporter,
        )
        .expect_err("no asset declared for the target must fail closed");
        assert!(
            error.to_string().contains("no hosted-release asset"),
            "{error}"
        );
    }

    #[test]
    fn fails_closed_when_no_assets_are_declared() {
        let root = TempDir::new().unwrap();
        let provider = provider_with_assets(vec![]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let mut reporter = RecordingReporter::new();

        let error = release_package(
            &request(root.path()),
            &document(),
            &providers,
            LINUX,
            None,
            &mut reporter,
        )
        .expect_err("no declared assets must fail closed");
        assert!(error.to_string().contains("nothing to package"), "{error}");
    }

    #[test]
    fn fails_closed_on_a_traversing_target_triple() {
        let root = TempDir::new().unwrap();
        let provider = provider_with_assets(vec!["dist/toven-x86_64-unknown-linux-gnu.tar.gz"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let mut reporter = RecordingReporter::new();

        let error = release_package(
            &request(root.path()),
            &document(),
            &providers,
            "../../etc",
            None,
            &mut reporter,
        )
        .expect_err("a traversing target triple must fail closed");
        assert!(
            error.to_string().contains("invalid target triple"),
            "{error}"
        );
    }
}
