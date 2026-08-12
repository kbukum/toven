use std::path::Path;

use rskit_errors::{AppError, AppResult};
use rskit_fs::TempDir;
use rskit_fs::sync_io::file::exists as file_exists;
use rskit_version::semver::Version;
use toven_ports::{AssetDownloader, Provider, Reporter, SignatureVerifier, VersionProbe};

use super::assets::{
    CERTIFICATE_NAME, MANIFEST_NAME, SIGNATURE_NAME, archive_assets, asset_file_name, binary_stem,
    build_tag, decide_version, digest_hex, extract_binary, parse_manifest, require_identity,
    safe_join_asset,
};
use crate::model::settings::ResolvedReleaseSettings;
use crate::planning::plan::{release_targets, resolve_release_settings};
use toven_core::config::Document;
use toven_core::federation::resolve::PathDriverLocator;
use toven_core::plan::{PlanRequest, prepare_front};

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

    let declared = crate::artifacts::assets::declared_release_assets(&settings);
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
