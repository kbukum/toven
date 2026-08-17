use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use rskit_errors::{AppError, AppResult};
use rskit_fs::TempDir;
use rskit_fs::sync_io::file::exists as file_exists;
use rskit_version::semver::Version;
use tokio_util::sync::CancellationToken;
use toven_ports::{AssetDownloader, Provider, Reporter, SignatureVerifier, VersionProbe};
use toven_runtime::{Completed, UnitOperation, UnitSpec};

use super::assets::{
    BUNDLE_NAME, MANIFEST_NAME, archive_assets, asset_file_name, binary_stem, build_tag,
    decide_version, digest_hex, extract_binary, parse_manifest, require_identity, safe_join_asset,
};
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
    readers: &toven_core::federation::baseline::MemberVcsReaders<'_>,
    options: VerifyOptions,
    downloader: &dyn AssetDownloader,
    verifier: &dyn SignatureVerifier,
    probe: &dyn VersionProbe,
    reporter: &mut dyn Reporter,
) -> AppResult<VerifyReport> {
    let inputs = VerifyInputs::gather(
        request, document, providers, readers, options, downloader, verifier, reporter,
    )?;
    let assets = inputs
        .archives
        .iter()
        .map(|unit| verify_for(&inputs, unit, probe))
        .collect::<AppResult<Vec<_>>>()?;
    Ok(inputs.report(assets))
}

/// One declared archive resolved to its owned verification unit during GATHER.
struct ArchiveUnit {
    /// The project-relative declared archive asset path (the unit id).
    asset: String,
    /// The archive's file name (its manifest key and downloaded file name).
    name: String,
}

/// The mode-specific shared state resolved once by [`VerifyInputs::gather`].
enum VerifyPlan {
    /// Local `dist/` verification: presence-check under the project root.
    Local {
        /// The project root the declared archives are resolved relative to.
        project_root: PathBuf,
    },
    /// Download verification: the fetched-and-signature-verified scratch dir and
    /// the now-trusted `SHA256SUMS` checksum map.
    Download {
        /// The scratch directory the hosted assets were downloaded into; kept
        /// alive so the per-unit checksum/extract phase can read them.
        scratch: TempDir,
        /// The trusted per-archive checksum map parsed from the signed manifest.
        manifest: std::collections::BTreeMap<String, String>,
    },
}

/// The shared prerequisites for `release verify`, resolved once by
/// [`VerifyInputs::gather`].
///
/// GATHER performs every workspace-coupled step: it decides the expected
/// version, resolves the declared archives, and — in download mode — fetches the
/// hosted assets and verifies the `SHA256SUMS` signature **before** any archive
/// is touched, so the fail-closed order (signature → per-archive checksum → run)
/// is preserved. The streamed per-unit phase then verifies one archive.
pub struct VerifyInputs {
    /// Which verification mode ran.
    mode: VerifyMode,
    /// The engine-decided version every archive must report.
    expected_version: Version,
    /// The hosted release tag consulted (download mode only).
    tag: Option<String>,
    /// Whether the packaged binary is executed per archive.
    run: bool,
    /// The declared archives, in declared order.
    archives: Vec<ArchiveUnit>,
    /// The mode-specific shared state.
    plan: VerifyPlan,
}

impl VerifyInputs {
    /// Resolve the expected version, the declared archives, and — in download
    /// mode — the downloaded, signature-verified assets and trusted manifest.
    ///
    /// # Errors
    /// Fails closed with a typed error when no archive asset is declared, the
    /// decided version is ambiguous, a download fails, the `SHA256SUMS`
    /// signature does not verify, or the keyless identity/issuer are
    /// unconfigured in download mode — as well as propagating configuration,
    /// discovery, graph, and I/O failures.
    #[allow(clippy::too_many_arguments)]
    pub fn gather(
        request: &PlanRequest,
        document: &Document,
        providers: &[&dyn Provider],
        readers: &toven_core::federation::baseline::MemberVcsReaders<'_>,
        options: VerifyOptions,
        downloader: &dyn AssetDownloader,
        verifier: &dyn SignatureVerifier,
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
        let archive_units = archives
            .iter()
            .map(|asset| {
                Ok(ArchiveUnit {
                    asset: (*asset).clone(),
                    name: asset_file_name(asset)?.to_string(),
                })
            })
            .collect::<AppResult<Vec<_>>>()?;

        let (mode, tag, plan) = if options.download {
            let identity = require_identity(
                representative,
                "identity",
                representative.sign.identity.as_deref(),
            )?;
            let issuer = require_identity(
                representative,
                "issuer",
                representative.sign.issuer.as_deref(),
            )?;
            let tag = build_tag(representative.tag_format.as_deref(), &expected_version);

            let scratch = TempDir::new()?;
            let dest = scratch.path();

            // Fetch the archives and the signed manifest + bundle by their file
            // names (the hosted release stores them flat).
            let archive_names: Vec<&str> = archive_units
                .iter()
                .map(|unit| unit.name.as_str())
                .collect();
            let mut wanted = archive_names.clone();
            wanted.extend([MANIFEST_NAME, BUNDLE_NAME]);
            downloader.download(&tag, &wanted, dest)?;

            // The checksums are only trustworthy once the keyless signature in
            // the manifest's bundle verifies against the configured workflow
            // identity — verify it once, up front, before any archive is read.
            verifier.verify_blob(
                &dest.join(MANIFEST_NAME),
                &dest.join(BUNDLE_NAME),
                identity,
                issuer,
            )?;
            let manifest = parse_manifest(&dest.join(MANIFEST_NAME))?;
            (
                VerifyMode::Download,
                Some(tag),
                VerifyPlan::Download { scratch, manifest },
            )
        } else {
            (
                VerifyMode::Local,
                None,
                VerifyPlan::Local {
                    project_root: request.project_root.as_path().to_path_buf(),
                },
            )
        };

        Ok(Self {
            mode,
            expected_version,
            tag,
            run: options.run,
            archives: archive_units,
            plan,
        })
    }

    /// Which verification mode this run performs.
    #[must_use]
    pub const fn mode(&self) -> VerifyMode {
        self.mode
    }

    /// The hosted release tag consulted (download mode) or `None` (local mode).
    #[must_use]
    pub fn tag(&self) -> Option<&str> {
        self.tag.as_deref()
    }

    /// The engine-decided version every archive must report.
    #[must_use]
    pub fn expected_version(&self) -> String {
        self.expected_version.to_string()
    }

    fn unit(&self, id: &str) -> Option<&ArchiveUnit> {
        self.archives.iter().find(|unit| unit.asset == id)
    }

    /// The engine unit graph: one independent unit per declared archive.
    #[must_use]
    pub fn units(&self) -> Vec<UnitSpec> {
        self.archives
            .iter()
            .map(|unit| UnitSpec::new(unit.asset.clone(), Vec::<String>::new()))
            .collect()
    }

    /// Assemble the terminal report from the per-archive outcomes — the
    /// post-stream aggregate.
    #[must_use]
    pub fn report(&self, assets: Vec<VerifiedAsset>) -> VerifyReport {
        VerifyReport {
            mode: self.mode,
            tag: self.tag.clone(),
            expected_version: self.expected_version.to_string(),
            assets,
        }
    }
}

/// One settled archive verification outcome.
pub type VerifyOutcome = VerifiedAsset;

/// Verify one declared archive — the pure per-unit compute over the gathered
/// [`VerifyInputs`]. Local mode presence-checks and (unless `--no-run`) runs the
/// packaged binary; download mode checksum-verifies the fetched archive against
/// the trusted manifest before extraction.
fn verify_for(
    inputs: &VerifyInputs,
    unit: &ArchiveUnit,
    probe: &dyn VersionProbe,
) -> AppResult<VerifyOutcome> {
    match &inputs.plan {
        VerifyPlan::Local { project_root } => {
            let path = safe_join_asset(project_root, &unit.asset)?;
            if !file_exists(&path)? {
                return Err(AppError::invalid_input(
                    "release.verify.asset",
                    format!(
                        "declared archive '{}' is not present; package it before verifying",
                        unit.asset
                    ),
                ));
            }
            let reported = if inputs.run {
                Some(extract_and_probe(&path, &inputs.expected_version, probe)?)
            } else {
                None
            };
            Ok(VerifiedAsset {
                name: unit.name.clone(),
                checksum_ok: None,
                signature_ok: None,
                ran: inputs.run,
                reported_version: reported,
            })
        }
        VerifyPlan::Download { scratch, manifest } => {
            let local_asset = scratch.path().join(&unit.name);
            let expected_hex = manifest.get(&unit.name).ok_or_else(|| {
                AppError::invalid_input(
                    "release.verify.checksum",
                    format!(
                        "manifest '{MANIFEST_NAME}' has no checksum entry for '{}'",
                        unit.name
                    ),
                )
            })?;
            let actual_hex = digest_hex(&local_asset)?;
            if &actual_hex != expected_hex {
                return Err(AppError::invalid_input(
                    "release.verify.checksum",
                    format!(
                        "checksum mismatch for '{}': manifest declares {expected_hex}, computed \
                         {actual_hex}",
                        unit.name
                    ),
                ));
            }
            let reported = if inputs.run {
                Some(extract_and_probe(
                    &local_asset,
                    &inputs.expected_version,
                    probe,
                )?)
            } else {
                None
            };
            Ok(VerifiedAsset {
                name: unit.name.clone(),
                checksum_ok: Some(true),
                signature_ok: Some(true),
                ran: inputs.run,
                reported_version: reported,
            })
        }
    }
}

/// The `release verify` per-unit operation on the shared runtime engine.
///
/// GATHER decides the version, resolves the archives, and — in download mode —
/// fetches and signature-verifies the hosted assets once into [`VerifyInputs`];
/// each unit streams one archive's checksum/presence + optional run. Extraction
/// and probing are synchronous port work, so each unit runs on a blocking
/// thread to let the engine schedule the archives bounded-parallel.
pub struct VerifyOperation {
    inputs: Arc<VerifyInputs>,
    probe: Arc<dyn VersionProbe>,
}

impl VerifyOperation {
    /// Wrap gathered inputs and the injected version probe as a runnable
    /// operation.
    #[must_use]
    pub fn new(inputs: VerifyInputs, probe: Arc<dyn VersionProbe>) -> Self {
        Self {
            inputs: Arc::new(inputs),
            probe,
        }
    }

    /// Share the gathered inputs so the CLI can title its output with the mode,
    /// tag, and expected version.
    #[must_use]
    pub fn inputs(&self) -> Arc<VerifyInputs> {
        Arc::clone(&self.inputs)
    }
}

#[async_trait]
impl UnitOperation for VerifyOperation {
    type Shared = Arc<VerifyInputs>;
    type Outcome = VerifyOutcome;

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
        let probe = Arc::clone(&self.probe);
        let id = unit_id.to_string();
        let outcome = tokio::task::spawn_blocking(move || {
            let unit = shared.unit(&id).ok_or_else(|| {
                AppError::new(
                    rskit_errors::ErrorCode::Internal,
                    format!("unknown verify unit '{id}'"),
                )
            })?;
            verify_for(&shared, unit, probe.as_ref())
        })
        .await
        .map_err(AppError::internal)??;
        Ok(Completed::succeeded(outcome))
    }
}

/// Build the `release verify` operation and its engine unit graph.
///
/// GATHER performs the shared download + signature verification (download mode)
/// once; the returned units stream the per-archive checksum/presence + run.
///
/// # Errors
/// Propagates GATHER failures (version decision, asset resolution, download,
/// signature verification).
#[allow(clippy::too_many_arguments)]
pub fn verify_operation(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    readers: &toven_core::federation::baseline::MemberVcsReaders<'_>,
    options: VerifyOptions,
    downloader: &dyn AssetDownloader,
    verifier: &dyn SignatureVerifier,
    probe: Arc<dyn VersionProbe>,
    reporter: &mut dyn Reporter,
) -> AppResult<(VerifyOperation, Vec<UnitSpec>)> {
    let inputs = VerifyInputs::gather(
        request, document, providers, readers, options, downloader, verifier, reporter,
    )?;
    let units = inputs.units();
    Ok((VerifyOperation::new(inputs, probe), units))
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
