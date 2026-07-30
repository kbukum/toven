//! `release verify` verb: check that the release artifacts a hosted release
//! declares are present, authentic, and run.
//!
//! Two modes, both non-mutating:
//!
//! - **local** (default) — for every declared archive asset, presence-check it,
//!   extract it via [`rskit_fs::archive`], run the packaged binary, and assert
//!   it reports the engine-decided release version. `--no-run` skips only the
//!   execution (for a cross-compiled target that cannot run on the verify
//!   runner), never the presence check.
//! - **download** — fetch each archive plus `SHA256SUMS` and its Sigstore
//!   signature/certificate from the hosted release, then, in a hard fail-closed
//!   order, **verify the signature on `SHA256SUMS` first**, **checksum-verify
//!   each archive against it next**, and only then extract/run/version-check.
//!
//! The engine owns verification *policy* — the fail-closed ordering, which
//! version is expected, and which assets are archives — while the ports do the
//! mechanical work: [`AssetDownloader`] fetches, [`SignatureVerifier`] runs the
//! keyless signature check (identity/issuer from config, never hard-coded),
//! [`VersionProbe`] runs the extracted binary, the §04 SHA-256 digest does the
//! checksum comparison in Rust (no shelling to `shasum`), and the §03 archive
//! primitive extracts. Every failure aborts with a typed [`AppError`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::archive::{ExtractLimits, extract_tar_gz, extract_zip};
use rskit_fs::sync_io::file::exists as file_exists;
use rskit_fs::{TempDir, safe_join};
use rskit_process::{CapturedIo, OutputPolicy, ProcessConfig, ProcessIo, ProcessSpec, run};
use rskit_util::hash::sha256::sha256_reader;
use rskit_version::semver::Version;
use toven_ports::{AssetDownloader, Provider, Reporter, SignatureVerifier, VersionProbe};

use super::plan::{release_targets, resolve_release_settings};
use super::settings::ResolvedReleaseSettings;
use crate::config::Document;
use crate::federation::resolve::PathDriverLocator;
use crate::plan::{PlanRequest, prepare_front};

/// The signed manifest and its Sigstore signature/certificate sidecars.
const MANIFEST_NAME: &str = "SHA256SUMS";
const SIGNATURE_NAME: &str = "SHA256SUMS.sig";
const CERTIFICATE_NAME: &str = "SHA256SUMS.pem";

/// The two archive extensions a declared asset can carry.
const TAR_GZ_EXT: &str = ".tar.gz";
const ZIP_EXT: &str = ".zip";

/// Timeout for a single `gh`/`cosign`/binary invocation. Downloads and keyless
/// verification round-trip to the forge and Fulcio/Rekor, so this is wider than
/// a local command.
const VERIFY_TIMEOUT: Duration = Duration::from_mins(5);

/// Hard bound on captured tool output (256 KiB) — guards against a pathological
/// stream while leaving room for a download progress log.
const MAX_VERIFY_OUTPUT_BYTES: usize = 256 * 1024;

/// Which verification mode ran.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub enum VerifyMode {
    /// Local `dist/` verification (presence + optional run).
    Local,
    /// Download-and-verify against the hosted release (signature + checksum +
    /// optional run).
    Download,
}

impl VerifyMode {
    /// The canonical mode name for typed reporting.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Download => "download",
        }
    }
}

/// What `release verify` was asked to do.
#[derive(Debug, Clone, Copy)]
pub struct VerifyOptions {
    /// Download the assets from the hosted release and verify signature +
    /// checksum before extraction, instead of verifying local `dist/` archives.
    pub download: bool,
    /// Run the extracted binary and assert its reported version. When `false`
    /// (`--no-run`), execution is skipped but presence/signature/checksum are
    /// still enforced.
    pub run: bool,
}

/// One archive's verification outcome.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct VerifiedAsset {
    /// The archive asset's file name.
    pub name: String,
    /// Whether the archive's checksum matched `SHA256SUMS`; `None` in local
    /// mode (no manifest is consulted).
    pub checksum_ok: Option<bool>,
    /// Whether the signature on `SHA256SUMS` verified; `None` in local mode.
    pub signature_ok: Option<bool>,
    /// Whether the packaged binary was executed.
    pub ran: bool,
    /// The version the packaged binary reported, when it was run.
    pub reported_version: Option<String>,
}

/// The typed result of `release verify`.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct VerifyReport {
    /// Which mode ran.
    pub mode: VerifyMode,
    /// The release tag consulted in download mode; `None` in local mode.
    pub tag: Option<String>,
    /// The engine-decided version every archive must report.
    pub expected_version: String,
    /// Per-archive verification outcomes, in declared order.
    pub assets: Vec<VerifiedAsset>,
}

/// Verify the declared release archives, locally or against the hosted release.
///
/// # Errors
/// Fails closed with a typed error when no archive asset is declared, the
/// decided version is ambiguous, a local archive is absent, a download fails,
/// the `SHA256SUMS` signature does not verify, an archive's checksum does not
/// match, the keyless identity/issuer are unconfigured in download mode, or a
/// packaged binary reports the wrong version — as well as propagating
/// configuration, discovery, graph, archive, and I/O failures.
#[allow(clippy::too_many_arguments)]
pub fn release_verify(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    options: VerifyOptions,
    downloader: &dyn AssetDownloader,
    verifier: &dyn SignatureVerifier,
    probe: &dyn VersionProbe,
    reporter: &mut dyn Reporter,
) -> AppResult<VerifyReport> {
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

    let expected_version = decide_version(&context, &targets, &settings)?;
    let representative = settings.values().next().ok_or_else(|| {
        AppError::invalid_input(
            "release.verify",
            "no releasable modules resolved; nothing to verify",
        )
    })?;

    let declared = declared_assets(&settings);
    let archives = archive_assets(&declared);
    if archives.is_empty() {
        return Err(AppError::invalid_input(
            "release.host.assets",
            "no archive assets are declared; nothing to verify (set […release.host].assets)",
        ));
    }

    if options.download {
        verify_download(
            request.project_root.as_path(),
            &archives,
            representative,
            &expected_version,
            options.run,
            downloader,
            verifier,
            probe,
        )
    } else {
        verify_local(
            request.project_root.as_path(),
            &archives,
            &expected_version,
            options.run,
            probe,
        )
    }
}

/// Verify local `dist/` archives: presence-check each, and (unless `--no-run`)
/// extract and assert the reported version.
fn verify_local(
    project_root: &Path,
    archives: &[&String],
    expected_version: &Version,
    run: bool,
    probe: &dyn VersionProbe,
) -> AppResult<VerifyReport> {
    let mut assets = Vec::with_capacity(archives.len());
    for archive in archives {
        let path = safe_join_asset(project_root, archive)?;
        if !file_exists(&path)? {
            return Err(AppError::invalid_input(
                "release.verify.asset",
                format!("declared archive '{archive}' is not present; package it before verifying"),
            ));
        }
        let reported = if run {
            Some(extract_and_probe(&path, expected_version, probe)?)
        } else {
            None
        };
        assets.push(VerifiedAsset {
            name: asset_file_name(archive)?.to_string(),
            checksum_ok: None,
            signature_ok: None,
            ran: run,
            reported_version: reported,
        });
    }
    Ok(VerifyReport {
        mode: VerifyMode::Local,
        tag: None,
        expected_version: expected_version.to_string(),
        assets,
    })
}

/// Download the assets and verify them in the hard fail-closed order:
/// signature on `SHA256SUMS` → per-archive checksum → extract/run.
#[allow(clippy::too_many_arguments)]
fn verify_download(
    project_root: &Path,
    archives: &[&String],
    settings: &ResolvedReleaseSettings,
    expected_version: &Version,
    run: bool,
    downloader: &dyn AssetDownloader,
    verifier: &dyn SignatureVerifier,
    probe: &dyn VersionProbe,
) -> AppResult<VerifyReport> {
    let _ = project_root;
    let identity = require_identity(settings, "identity", settings.sign.identity.as_deref())?;
    let issuer = require_identity(settings, "issuer", settings.sign.issuer.as_deref())?;
    let tag = build_tag(settings.tag_format.as_deref(), expected_version);

    let scratch = TempDir::new()?;
    let dest = scratch.path();

    // Fetch the archives and the signed manifest + sidecars by their file names
    // (the hosted release stores them flat), matching the manifest's own naming.
    let archive_names: Vec<&str> = archives
        .iter()
        .map(|asset| asset_file_name(asset))
        .collect::<AppResult<Vec<_>>>()?;
    let mut wanted = archive_names.clone();
    wanted.extend([MANIFEST_NAME, SIGNATURE_NAME, CERTIFICATE_NAME]);
    downloader.download(&tag, &wanted, dest)?;

    // 1. The checksums are only trustworthy once the keyless signature on the
    //    manifest itself verifies against the configured workflow identity.
    verifier.verify_blob(
        &dest.join(MANIFEST_NAME),
        &dest.join(SIGNATURE_NAME),
        &dest.join(CERTIFICATE_NAME),
        identity,
        issuer,
    )?;

    // 2. Parse the now-trusted manifest and checksum-verify every archive
    //    against it before touching its contents.
    let manifest = parse_manifest(&dest.join(MANIFEST_NAME))?;

    let mut assets = Vec::with_capacity(archives.len());
    for (asset, name) in archives.iter().zip(&archive_names) {
        let local_asset = dest.join(name);
        let expected_hex = manifest.get(*name).ok_or_else(|| {
            AppError::invalid_input(
                "release.verify.checksum",
                format!("manifest '{MANIFEST_NAME}' has no checksum entry for '{name}'"),
            )
        })?;
        let actual_hex = digest_hex(&local_asset)?;
        if &actual_hex != expected_hex {
            return Err(AppError::invalid_input(
                "release.verify.checksum",
                format!(
                    "checksum mismatch for '{name}': manifest declares {expected_hex}, computed \
                     {actual_hex}"
                ),
            ));
        }
        // 3. Only now, with a verified signature and checksum, extract and run.
        let reported = if run {
            Some(extract_and_probe(&local_asset, expected_version, probe)?)
        } else {
            None
        };
        assets.push(VerifiedAsset {
            name: asset_file_name(asset)?.to_string(),
            checksum_ok: Some(true),
            signature_ok: Some(true),
            ran: run,
            reported_version: reported,
        });
    }

    Ok(VerifyReport {
        mode: VerifyMode::Download,
        tag: Some(tag),
        expected_version: expected_version.to_string(),
        assets,
    })
}

/// Extract the single-binary archive at `archive` into a scratch directory, run
/// the packaged binary, and assert it reports `<stem> <expected_version>`.
fn extract_and_probe(
    archive: &Path,
    expected_version: &Version,
    probe: &dyn VersionProbe,
) -> AppResult<String> {
    let scratch = TempDir::new()?;
    let binary = extract_binary(archive, scratch.path())?;
    let reported = probe.report_version(&binary)?;
    let stem = binary_stem(&binary)?;
    let expected = format!("{stem} {expected_version}");
    if reported.trim() != expected {
        return Err(AppError::invalid_input(
            "release.verify.version",
            format!(
                "packaged binary in '{}' reported '{}', expected '{expected}'",
                archive.display(),
                reported.trim()
            ),
        ));
    }
    Ok(reported.trim().to_string())
}

/// Extract the archive at `archive` into `dest` and return the single packaged
/// binary member.
fn extract_binary(archive: &Path, dest: &Path) -> AppResult<PathBuf> {
    let name = asset_file_name_path(archive)?;
    let extracted = if name.ends_with(ZIP_EXT) {
        extract_zip(archive, dest, ExtractLimits::default())?
    } else if name.ends_with(TAR_GZ_EXT) {
        extract_tar_gz(archive, dest, ExtractLimits::default())?
    } else {
        return Err(AppError::invalid_input(
            "release.verify.archive",
            format!(
                "archive '{}' is neither a .tar.gz nor a .zip",
                archive.display()
            ),
        ));
    };
    extracted.into_iter().next().ok_or_else(|| {
        AppError::invalid_input(
            "release.verify.archive",
            format!(
                "archive '{}' contained no packaged binary",
                archive.display()
            ),
        )
    })
}

/// The binary's program name for the expected version line: its file name with
/// any `.exe` extension stripped.
fn binary_stem(binary: &Path) -> AppResult<String> {
    let name = binary
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            AppError::new(
                ErrorCode::Internal,
                format!("extracted binary '{}' has no file name", binary.display()),
            )
        })?;
    Ok(name.strip_suffix(".exe").unwrap_or(name).to_string())
}

/// Decide the single version every releasable module must report. The locked
/// same-version-per-kit policy means every module declares the same version; a
/// disagreement is a fail-closed error rather than a silent pick.
fn decide_version(
    context: &crate::plan::PlanContext,
    targets: &super::ReleaseTargets,
    settings: &BTreeMap<toven_model::ModuleKey, ResolvedReleaseSettings>,
) -> AppResult<Version> {
    let mut decided: Option<Version> = None;
    for module in &context.federation.modules {
        let key = (module.member.clone(), module.id.ecosystem.clone());
        let Some(target) = targets.get(&key) else {
            continue;
        };
        let Some(resolved) = settings.get(&module.key()) else {
            continue;
        };
        if !resolved.publication.releases() {
            continue;
        }
        let declared = target.declared_version(module)?;
        match &decided {
            None => decided = Some(declared),
            Some(existing) if existing != &declared => {
                return Err(AppError::invalid_input(
                    "release.verify.version",
                    format!(
                        "releasable modules declare divergent versions ({existing} vs \
                         {declared}); cannot decide a single expected version"
                    ),
                ));
            }
            Some(_) => {}
        }
    }
    decided.ok_or_else(|| {
        AppError::invalid_input(
            "release.verify",
            "no releasable module declares a version to verify against",
        )
    })
}

/// The sorted, de-duplicated union of every module's declared hosted-release
/// assets.
fn declared_assets(
    settings: &BTreeMap<toven_model::ModuleKey, ResolvedReleaseSettings>,
) -> Vec<&String> {
    let mut assets: Vec<&String> = settings
        .values()
        .flat_map(|resolved| resolved.host.assets.iter())
        .collect();
    assets.sort();
    assets.dedup();
    assets
}

/// The declared assets that are archives (`.tar.gz` / `.zip`), in declared
/// order.
fn archive_assets<'a>(declared: &[&'a String]) -> Vec<&'a String> {
    declared
        .iter()
        .filter(|asset| {
            asset_file_name(asset)
                .is_ok_and(|name| name.ends_with(TAR_GZ_EXT) || name.ends_with(ZIP_EXT))
        })
        .copied()
        .collect()
}

/// Parse a `SHA256SUMS` body (`shasum -a 256` two-space format) into a
/// name → lowercase-hex map.
fn parse_manifest(path: &Path) -> AppResult<BTreeMap<String, String>> {
    let bytes = rskit_fs::sync_io::file::read(path)?;
    let text = String::from_utf8(bytes).map_err(|error| {
        AppError::new(
            ErrorCode::InvalidInput,
            format!("manifest '{}' is not valid UTF-8: {error}", path.display()),
        )
    })?;
    let mut entries = BTreeMap::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let (hex, name) = line.split_once("  ").ok_or_else(|| {
            AppError::invalid_input(
                "release.verify.checksum",
                format!("malformed manifest line '{line}' (expected '<hex>  <name>')"),
            )
        })?;
        entries.insert(name.to_string(), hex.to_string());
    }
    Ok(entries)
}

/// The lowercase-hex SHA-256 digest of the file at `path`.
fn digest_hex(path: &Path) -> AppResult<String> {
    let mut file = std::fs::File::open(path).map_err(|error| {
        AppError::new(
            ErrorCode::Internal,
            format!("cannot open '{}' for checksum: {error}", path.display()),
        )
        .with_cause(error)
    })?;
    Ok(sha256_reader(&mut file)?.to_hex())
}

/// Require a configured keyless verification field (identity/issuer).
fn require_identity<'a>(
    _settings: &ResolvedReleaseSettings,
    field: &str,
    value: Option<&'a str>,
) -> AppResult<&'a str> {
    value.ok_or_else(|| {
        AppError::invalid_input(
            format!("release.sign.{field}"),
            format!("download verification needs the keyless {field}; set […release.sign].{field}"),
        )
    })
}

/// Build the release tag from the configured tag format (default `v{version}`)
/// by substituting the decided version.
#[allow(clippy::literal_string_with_formatting_args)]
fn build_tag(tag_format: Option<&str>, version: &Version) -> String {
    tag_format
        .unwrap_or("v{version}")
        .replace("{version}", &version.to_string())
}

/// Resolve a declared project-relative asset to an absolute path, mapping a
/// traversing path to a typed error.
fn safe_join_asset(project_root: &Path, asset: &str) -> AppResult<PathBuf> {
    safe_join(project_root, asset).map_err(|error| {
        AppError::invalid_input(
            "release.host.assets",
            format!("asset '{asset}' is not a safe project-relative path"),
        )
        .with_cause(error)
    })
}

/// The final path component of a project-relative asset path.
fn asset_file_name(asset: &str) -> AppResult<&str> {
    Path::new(asset)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            AppError::invalid_input(
                "release.host.assets",
                format!("asset '{asset}' has no file name"),
            )
        })
}

/// The final path component of an on-disk path as a string.
fn asset_file_name_path(path: &Path) -> AppResult<&str> {
    path.file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            AppError::new(
                ErrorCode::Internal,
                format!("path '{}' has no file name", path.display()),
            )
        })
}

/// Run an argv-only external tool with a bounded, captured output and the shared
/// verify timeout, mapping a spawn/exec/non-zero failure to a typed error.
fn run_tool(program: &str, argv: Vec<String>, cwd: Option<&Path>) -> AppResult<String> {
    let mut spec = ProcessSpec::new(program).args(argv);
    if let Some(cwd) = cwd {
        spec = spec.dir(cwd);
    }
    let config = ProcessConfig::default()
        .with_timeout(Some(VERIFY_TIMEOUT))
        .with_io(ProcessIo::captured(CapturedIo::new().with_output(
            OutputPolicy::captured().with_max_output_bytes(MAX_VERIFY_OUTPUT_BYTES),
        )));
    let result = run(&spec, &config)?;
    result.check()?;
    Ok(result.stdout)
}

/// [`AssetDownloader`] backed by `gh release download`, argv-only through
/// [`rskit_process`]. Authentication stays ambient (the runner's `gh`
/// credentials); no token is placed on argv or captured.
#[derive(Debug, Clone, Default)]
pub struct GhAssetDownloader;

impl GhAssetDownloader {
    /// Construct a `gh`-backed downloader.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl AssetDownloader for GhAssetDownloader {
    fn download(&self, tag: &str, assets: &[&str], dest: &Path) -> AppResult<()> {
        let mut argv = vec![
            "release".to_string(),
            "download".to_string(),
            tag.to_string(),
            "--dir".to_string(),
            path_arg(dest)?,
        ];
        for asset in assets {
            argv.push("--pattern".to_string());
            argv.push((*asset).to_string());
        }
        run_tool("gh", argv, None)?;
        Ok(())
    }
}

/// [`SignatureVerifier`] backed by `cosign verify-blob`, argv-only through
/// [`rskit_process`].
///
/// Keyless verification checks the transparency-log entry and certificate chain
/// against the configured identity/issuer. No secret is involved — verification
/// is against public Sigstore state.
#[derive(Debug, Clone, Default)]
pub struct CosignVerifier;

impl CosignVerifier {
    /// Construct a cosign-backed signature verifier.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl SignatureVerifier for CosignVerifier {
    fn verify_blob(
        &self,
        blob: &Path,
        signature: &Path,
        certificate: &Path,
        identity: &str,
        issuer: &str,
    ) -> AppResult<()> {
        let argv = vec![
            "verify-blob".to_string(),
            "--certificate".to_string(),
            path_arg(certificate)?,
            "--signature".to_string(),
            path_arg(signature)?,
            "--certificate-identity-regexp".to_string(),
            identity.to_string(),
            "--certificate-oidc-issuer".to_string(),
            issuer.to_string(),
            path_arg(blob)?,
        ];
        run_tool("cosign", argv, None)?;
        Ok(())
    }
}

/// [`VersionProbe`] that runs `<binary> --version` argv-only through
/// [`rskit_process`] and returns the stdout it prints.
#[derive(Debug, Clone, Default)]
pub struct ProcessVersionProbe;

impl ProcessVersionProbe {
    /// Construct a process-backed version probe.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl VersionProbe for ProcessVersionProbe {
    fn report_version(&self, binary: &Path) -> AppResult<String> {
        run_tool(&path_arg(binary)?, vec!["--version".to_string()], None)
    }
}

/// Render a path as a UTF-8 argv string, failing closed on a non-UTF-8 path so
/// nothing lossy reaches an external tool.
fn path_arg(path: &Path) -> AppResult<String> {
    path.to_str().map(str::to_string).ok_or_else(|| {
        AppError::new(
            ErrorCode::Internal,
            format!("path '{}' is not valid UTF-8", path.display()),
        )
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use rskit_config::RawValue;
    use rskit_fs::TempDir;
    use rskit_fs::archive::{ArchiveEntry, tar_gz};
    use rskit_version::semver::Version;
    use serde_json::json;
    use toven_model::{AbsPath, EcosystemId, Module, ModuleRef, RepoPath};
    use toven_ports::{
        CommonEcosystemConfig, DiscoverResponse, HostConfig, Provider, ReleaseConfig, SignConfig,
        TaskIntent,
    };
    use toven_testkit::{
        FakeAssetDownloader, FakeConfiguredAdapter, FakeProvider, FakeReleaseTarget,
        FakeSignatureVerifier, FakeVersionProbe, RecordingReporter,
    };

    use super::{VerifyMode, VerifyOptions, release_verify};
    use crate::config::{Document, ProjectConfig, TovenConfig};
    use crate::plan::PlanRequest;

    const LINUX_ARCHIVE: &str = "dist/toven-x86_64-unknown-linux-gnu.tar.gz";
    const ARCHIVE_NAME: &str = "toven-x86_64-unknown-linux-gnu.tar.gz";
    const VERSION: &str = "0.4.2";

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

    /// A provider whose ecosystem declares `assets` and the given `sign` config,
    /// with a release target reporting `VERSION`.
    fn provider(assets: Vec<&str>, sign: SignConfig) -> FakeProvider {
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![module("cli")];
        let common = CommonEcosystemConfig {
            release: ReleaseConfig {
                host: Some(HostConfig {
                    forge: Some("github".to_string()),
                    assets: Some(assets.into_iter().map(str::to_string).collect()),
                    ..HostConfig::default()
                }),
                sign: Some(sign),
                ..ReleaseConfig::default()
            },
            ..CommonEcosystemConfig::default()
        };
        let target =
            FakeReleaseTarget::new().with_declared_version(Version::parse(VERSION).unwrap());
        let adapter = FakeConfiguredAdapter::new(eid("rust"))
            .with_response(response)
            .with_release_target(target)
            .with_common(common);
        FakeProvider::new(eid("rust")).with_adapter(adapter)
    }

    /// Write a deterministic `toven` archive at `root/dist/<ARCHIVE_NAME>`
    /// carrying a single binary member named `toven`.
    fn write_archive(root: &Path) {
        let dir = root.join("dist");
        std::fs::create_dir_all(&dir).unwrap();
        let binary = dir.join("toven-binary");
        std::fs::write(&binary, b"fake-toven").unwrap();
        let entries = [ArchiveEntry::new("toven", &binary, 0o755)];
        tar_gz(&entries, &dir.join(ARCHIVE_NAME)).unwrap();
        std::fs::remove_file(&binary).unwrap();
    }

    #[test]
    fn local_verify_presence_and_version() {
        let root = TempDir::new().unwrap();
        write_archive(root.path());
        let provider = provider(vec![LINUX_ARCHIVE], SignConfig::default());
        let providers: Vec<&dyn Provider> = vec![&provider];
        let mut reporter = RecordingReporter::new();
        let downloader = FakeAssetDownloader::from_dir(root.path());
        let verifier = FakeSignatureVerifier::new();
        let probe = FakeVersionProbe::reporting(format!("toven {VERSION}"));

        let report = release_verify(
            &request(root.path()),
            &document(),
            &providers,
            VerifyOptions {
                download: false,
                run: true,
            },
            &downloader,
            &verifier,
            &probe,
            &mut reporter,
        )
        .unwrap();

        assert_eq!(report.mode, VerifyMode::Local);
        assert_eq!(report.expected_version, VERSION);
        assert_eq!(report.assets.len(), 1);
        assert_eq!(report.assets[0].name, ARCHIVE_NAME);
        assert!(report.assets[0].ran);
        assert_eq!(
            report.assets[0].reported_version.as_deref(),
            Some(&*format!("toven {VERSION}"))
        );
        assert_eq!(report.assets[0].checksum_ok, None);
    }

    #[test]
    fn local_no_run_skips_execution_but_checks_presence() {
        let root = TempDir::new().unwrap();
        write_archive(root.path());
        let provider = provider(vec![LINUX_ARCHIVE], SignConfig::default());
        let providers: Vec<&dyn Provider> = vec![&provider];
        let mut reporter = RecordingReporter::new();
        let downloader = FakeAssetDownloader::from_dir(root.path());
        let verifier = FakeSignatureVerifier::new();
        let probe = FakeVersionProbe::failing("must not run");

        let report = release_verify(
            &request(root.path()),
            &document(),
            &providers,
            VerifyOptions {
                download: false,
                run: false,
            },
            &downloader,
            &verifier,
            &probe,
            &mut reporter,
        )
        .unwrap();

        assert!(!report.assets[0].ran);
        assert!(report.assets[0].reported_version.is_none());
        assert!(probe.probed().is_empty(), "the binary must not be executed");
    }

    #[test]
    fn local_verify_fails_closed_on_missing_archive() {
        let root = TempDir::new().unwrap();
        // No archive written.
        let provider = provider(vec![LINUX_ARCHIVE], SignConfig::default());
        let providers: Vec<&dyn Provider> = vec![&provider];
        let mut reporter = RecordingReporter::new();
        let downloader = FakeAssetDownloader::from_dir(root.path());
        let verifier = FakeSignatureVerifier::new();
        let probe = FakeVersionProbe::reporting(format!("toven {VERSION}"));

        let error = release_verify(
            &request(root.path()),
            &document(),
            &providers,
            VerifyOptions {
                download: false,
                run: true,
            },
            &downloader,
            &verifier,
            &probe,
            &mut reporter,
        )
        .expect_err("a missing archive must fail closed");
        assert!(error.to_string().contains("is not present"), "{error}");
    }

    #[test]
    fn local_verify_fails_closed_on_wrong_reported_version() {
        let root = TempDir::new().unwrap();
        write_archive(root.path());
        let provider = provider(vec![LINUX_ARCHIVE], SignConfig::default());
        let providers: Vec<&dyn Provider> = vec![&provider];
        let mut reporter = RecordingReporter::new();
        let downloader = FakeAssetDownloader::from_dir(root.path());
        let verifier = FakeSignatureVerifier::new();
        let probe = FakeVersionProbe::reporting("toven 9.9.9");

        let error = release_verify(
            &request(root.path()),
            &document(),
            &providers,
            VerifyOptions {
                download: false,
                run: true,
            },
            &downloader,
            &verifier,
            &probe,
            &mut reporter,
        )
        .expect_err("a wrong reported version must fail closed");
        assert!(
            error.to_string().contains("expected 'toven 0.4.2'"),
            "{error}"
        );
    }

    /// Stage a "remote" directory holding the archive, a correct `SHA256SUMS`,
    /// and signature/certificate sidecars, returning its path (kept alive by the
    /// returned `TempDir`).
    fn stage_remote(tamper: bool) -> (TempDir, String) {
        let remote = TempDir::new().unwrap();
        let binary = remote.path().join("toven-binary");
        std::fs::write(&binary, b"fake-toven").unwrap();
        let entries = [ArchiveEntry::new("toven", &binary, 0o755)];
        tar_gz(&entries, &remote.path().join(ARCHIVE_NAME)).unwrap();
        std::fs::remove_file(&binary).unwrap();

        let digest = if tamper {
            "0".repeat(64)
        } else {
            let mut file = std::fs::File::open(remote.path().join(ARCHIVE_NAME)).unwrap();
            rskit_util::hash::sha256::sha256_reader(&mut file)
                .unwrap()
                .to_hex()
        };
        std::fs::write(
            remote.path().join("SHA256SUMS"),
            format!("{digest}  {ARCHIVE_NAME}\n"),
        )
        .unwrap();
        std::fs::write(remote.path().join("SHA256SUMS.sig"), b"sig").unwrap();
        std::fs::write(remote.path().join("SHA256SUMS.pem"), b"pem").unwrap();
        let path = remote.path().to_str().unwrap().to_string();
        (remote, path)
    }

    fn signed_provider() -> FakeProvider {
        let sign = SignConfig {
            enabled: true,
            signer: None,
            identity: Some(
                "https://github.com/kbukum/toven/.github/workflows/release.yml@.*".to_string(),
            ),
            issuer: Some("https://token.actions.githubusercontent.com".to_string()),
        };
        provider(vec![LINUX_ARCHIVE], sign)
    }

    #[test]
    fn download_verify_signature_then_checksum_then_run() {
        let (_remote, remote_path) = stage_remote(false);
        let root = TempDir::new().unwrap();
        let provider = signed_provider();
        let providers: Vec<&dyn Provider> = vec![&provider];
        let mut reporter = RecordingReporter::new();
        let downloader = FakeAssetDownloader::from_dir(remote_path);
        let verifier = FakeSignatureVerifier::new();
        let probe = FakeVersionProbe::reporting(format!("toven {VERSION}"));

        let report = release_verify(
            &request(root.path()),
            &document(),
            &providers,
            VerifyOptions {
                download: true,
                run: true,
            },
            &downloader,
            &verifier,
            &probe,
            &mut reporter,
        )
        .unwrap();

        assert_eq!(report.mode, VerifyMode::Download);
        assert_eq!(report.tag.as_deref(), Some(&*format!("v{VERSION}")));
        assert_eq!(report.assets[0].checksum_ok, Some(true));
        assert_eq!(report.assets[0].signature_ok, Some(true));
        assert!(report.assets[0].ran);
        // The signature was verified against the configured keyless identity.
        assert_eq!(verifier.calls().len(), 1);
        assert!(
            verifier.calls()[0]
                .issuer
                .contains("token.actions.githubusercontent.com")
        );
    }

    #[test]
    fn download_verify_aborts_before_checksum_on_bad_signature() {
        let (_remote, remote_path) = stage_remote(false);
        let root = TempDir::new().unwrap();
        let provider = signed_provider();
        let providers: Vec<&dyn Provider> = vec![&provider];
        let mut reporter = RecordingReporter::new();
        let downloader = FakeAssetDownloader::from_dir(remote_path);
        let verifier = FakeSignatureVerifier::failing("signature does not verify");
        let probe = FakeVersionProbe::failing("must not run");

        let error = release_verify(
            &request(root.path()),
            &document(),
            &providers,
            VerifyOptions {
                download: true,
                run: true,
            },
            &downloader,
            &verifier,
            &probe,
            &mut reporter,
        )
        .expect_err("a bad signature must abort");
        assert!(error.to_string().contains("does not verify"), "{error}");
        assert!(
            probe.probed().is_empty(),
            "must not extract/run after a bad signature"
        );
    }

    #[test]
    fn download_verify_fails_closed_on_tampered_checksum() {
        let (_remote, remote_path) = stage_remote(true);
        let root = TempDir::new().unwrap();
        let provider = signed_provider();
        let providers: Vec<&dyn Provider> = vec![&provider];
        let mut reporter = RecordingReporter::new();
        let downloader = FakeAssetDownloader::from_dir(remote_path);
        let verifier = FakeSignatureVerifier::new();
        let probe = FakeVersionProbe::failing("must not run");

        let error = release_verify(
            &request(root.path()),
            &document(),
            &providers,
            VerifyOptions {
                download: true,
                run: true,
            },
            &downloader,
            &verifier,
            &probe,
            &mut reporter,
        )
        .expect_err("a tampered checksum must fail closed");
        assert!(error.to_string().contains("checksum mismatch"), "{error}");
        assert!(
            probe.probed().is_empty(),
            "must not run a checksum-failing archive"
        );
    }

    #[test]
    fn download_no_run_still_verifies_signature_and_checksum() {
        let (_remote, remote_path) = stage_remote(false);
        let root = TempDir::new().unwrap();
        let provider = signed_provider();
        let providers: Vec<&dyn Provider> = vec![&provider];
        let mut reporter = RecordingReporter::new();
        let downloader = FakeAssetDownloader::from_dir(remote_path);
        let verifier = FakeSignatureVerifier::new();
        let probe = FakeVersionProbe::failing("must not run");

        let report = release_verify(
            &request(root.path()),
            &document(),
            &providers,
            VerifyOptions {
                download: true,
                run: false,
            },
            &downloader,
            &verifier,
            &probe,
            &mut reporter,
        )
        .unwrap();

        assert_eq!(report.assets[0].checksum_ok, Some(true));
        assert_eq!(report.assets[0].signature_ok, Some(true));
        assert!(!report.assets[0].ran);
        assert_eq!(verifier.calls().len(), 1);
        assert!(probe.probed().is_empty());
    }

    #[test]
    fn download_fails_closed_when_identity_unconfigured() {
        let (_remote, remote_path) = stage_remote(false);
        let root = TempDir::new().unwrap();
        // Signing enabled but no identity/issuer configured.
        let sign = SignConfig {
            enabled: true,
            ..SignConfig::default()
        };
        let provider = provider(vec![LINUX_ARCHIVE], sign);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let mut reporter = RecordingReporter::new();
        let downloader = FakeAssetDownloader::from_dir(remote_path);
        let verifier = FakeSignatureVerifier::new();
        let probe = FakeVersionProbe::reporting(format!("toven {VERSION}"));

        let error = release_verify(
            &request(root.path()),
            &document(),
            &providers,
            VerifyOptions {
                download: true,
                run: true,
            },
            &downloader,
            &verifier,
            &probe,
            &mut reporter,
        )
        .expect_err("download verification needs a configured identity");
        assert!(error.to_string().contains("keyless identity"), "{error}");
    }
}
