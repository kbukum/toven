//! Streamed human and JSONL projectors for the artifact actions.

use rskit_cli::OutputTable;
use rskit_errors::{AppError, AppResult};
use serde::Serialize;
use toven_model::ModuleKey;
use toven_release::{
    ArtifactManifest, ChecksumOutcome, DepgraphOutcome, ImageModuleOutcome, PackageOutcome,
    ProvenanceOutcome, SbomOutcome, SignOutcome, StagedSbom, VerifyOutcome,
};

use crate::flags::OutputKind;

use super::summary::render_optional_check;

/// A stable JSON-lines record for one generated SBOM artifact.
#[derive(Serialize)]
struct SbomRecord {
    module: String,
    path: String,
}

/// A stable JSON-lines record for one staged SBOM release asset.
#[derive(Serialize)]
struct StagedSbomRecord {
    asset: String,
    source: String,
}

/// Streams `release sbom` per module as each artifact settles on the runtime
/// engine, never buffering a terminal table before first output.
///
/// Human mode prints one row per produced artifact to stdout under a
/// deterministic `Release SBOM artifacts` header (rows stream in completion
/// order, matched order-insensitively); an `examining` line goes to stderr for
/// live motion. JSONL mode emits one [`SbomRecord`] per artifact to stdout. The
/// staged hosted assets and skipped modules are the trailing aggregate emitted
/// by [`SbomProjector::finish`].
pub(super) struct SbomProjector {
    pub(super) output: OutputKind,
    /// The produced artifacts, accumulated in completion order for staging.
    pub(super) artifacts: Vec<ArtifactManifest>,
    /// Modules skipped because their ecosystem has no SBOM tooling.
    pub(super) skipped: Vec<ModuleKey>,
}

impl SbomProjector {
    pub(super) const fn new(output: OutputKind) -> Self {
        Self {
            output,
            artifacts: Vec::new(),
            skipped: Vec::new(),
        }
    }

    /// Print the deterministic human header once, before any module streams.
    pub(super) fn open(&self) {
        if matches!(self.output, OutputKind::Human) {
            println!("Release SBOM artifacts");
        }
    }

    /// Emit the trailing aggregate: staged hosted assets, then skip warnings.
    pub(super) fn finish(&self, staged: &[StagedSbom]) -> AppResult<()> {
        match self.output {
            OutputKind::Jsonl => {
                for asset in staged {
                    let record = StagedSbomRecord {
                        asset: asset.asset.clone(),
                        source: asset.source.clone(),
                    };
                    let line = serde_json::to_string(&record).map_err(AppError::internal)?;
                    println!("{line}");
                }
            }
            OutputKind::Human => {
                if !staged.is_empty() {
                    let mut table =
                        OutputTable::new(vec!["Asset", "Source"]).with_title("Staged SBOM assets");
                    for asset in staged {
                        table.add_row(vec![asset.asset.clone(), asset.source.clone()]);
                    }
                    println!("{table}");
                }
            }
        }
        for module in &self.skipped {
            eprintln!("warning: {module} skipped (ecosystem has no SBOM tooling)");
        }
        Ok(())
    }
}

impl toven_runtime::Progress<SbomOutcome> for SbomProjector {
    fn started(&mut self, unit_id: &str) -> AppResult<()> {
        if matches!(self.output, OutputKind::Human) {
            eprintln!("examining {unit_id}");
        }
        Ok(())
    }

    fn settled(&mut self, report: &toven_runtime::UnitReport<SbomOutcome>) -> AppResult<()> {
        let Some(outcome) = &report.outcome else {
            return Ok(());
        };
        match &outcome.artifact {
            Some(artifact) => {
                match self.output {
                    OutputKind::Jsonl => {
                        let record = SbomRecord {
                            module: artifact.label.clone(),
                            path: artifact.path.display().to_string(),
                        };
                        let line = serde_json::to_string(&record).map_err(AppError::internal)?;
                        println!("{line}");
                    }
                    OutputKind::Human => {
                        println!("{}  {}", artifact.label, artifact.path.display());
                    }
                }
                self.artifacts.push(artifact.clone());
            }
            None => self.skipped.push(outcome.module.clone()),
        }
        Ok(())
    }
}

/// A stable JSON-lines record for one generated dependency-graph artifact.
#[derive(Serialize)]
struct DepgraphRecord {
    label: String,
    path: String,
}

pub(super) struct DepgraphProjector {
    pub(super) output: OutputKind,
}

impl DepgraphProjector {
    pub(super) const fn new(output: OutputKind) -> Self {
        Self { output }
    }

    pub(super) fn open(&self) {
        if matches!(self.output, OutputKind::Human) {
            println!("Release dependency graphs");
        }
    }
}

impl toven_runtime::Progress<DepgraphOutcome> for DepgraphProjector {
    fn started(&mut self, unit_id: &str) -> AppResult<()> {
        if matches!(self.output, OutputKind::Human) {
            eprintln!("examining {unit_id}");
        }
        Ok(())
    }

    fn settled(&mut self, report: &toven_runtime::UnitReport<DepgraphOutcome>) -> AppResult<()> {
        let Some(artifact) = &report.outcome else {
            return Ok(());
        };
        match self.output {
            OutputKind::Jsonl => {
                let record = DepgraphRecord {
                    label: artifact.label.clone(),
                    path: artifact.path.display().to_string(),
                };
                let line = serde_json::to_string(&record).map_err(AppError::internal)?;
                println!("{line}");
            }
            OutputKind::Human => println!("{}  {}", artifact.label, artifact.path.display()),
        }
        Ok(())
    }
}

/// A stable JSON-lines record for one packaged release archive.
#[derive(Serialize)]
struct PackageRecord {
    target: String,
    asset: String,
    source: String,
    format: String,
    bytes: u64,
    backing: String,
}

/// Streams `release package` per declared asset as each archive settles on the
/// runtime engine, never buffering a terminal aggregate before first output.
///
/// Human mode prints a `Release packages (<target>)` header once, then one row
/// per asset to stdout as it settles; a `packaging` line goes to stderr for live
/// motion. JSONL mode emits one [`PackageRecord`] per asset to stdout.
pub(super) struct PackageProjector {
    pub(super) output: OutputKind,
    pub(super) target: String,
}

impl PackageProjector {
    pub(super) const fn new(output: OutputKind, target: String) -> Self {
        Self { output, target }
    }

    pub(super) fn open(&self) {
        if matches!(self.output, OutputKind::Human) {
            println!("Release packages ({})", self.target);
        }
    }
}

impl toven_runtime::Progress<PackageOutcome> for PackageProjector {
    fn started(&mut self, unit_id: &str) -> AppResult<()> {
        if matches!(self.output, OutputKind::Human) {
            eprintln!("packaging {unit_id}");
        }
        Ok(())
    }

    fn settled(&mut self, report: &toven_runtime::UnitReport<PackageOutcome>) -> AppResult<()> {
        let Some(asset) = &report.outcome else {
            return Ok(());
        };
        match self.output {
            OutputKind::Jsonl => {
                let record = PackageRecord {
                    target: self.target.clone(),
                    asset: asset.asset.clone(),
                    source: asset.source.display().to_string(),
                    format: asset.format.as_str().to_string(),
                    bytes: asset.bytes,
                    backing: asset.backing.to_string(),
                };
                let line = serde_json::to_string(&record).map_err(AppError::internal)?;
                println!("{line}");
            }
            OutputKind::Human => println!(
                "{}  {}  {}  {}  {}",
                asset.asset,
                asset.source.display(),
                asset.format.as_str(),
                asset.bytes,
                asset.backing
            ),
        }
        Ok(())
    }
}

/// A stable JSON-lines record for one checksummed release asset.
#[derive(Serialize)]
struct ChecksumRecord {
    manifest: String,
    name: String,
    sha256: String,
    bytes: u64,
}

pub(super) struct ChecksumProjector {
    pub(super) output: OutputKind,
    pub(super) manifest: String,
    pub(super) entries: Vec<ChecksumOutcome>,
}

impl ChecksumProjector {
    pub(super) const fn new(output: OutputKind, manifest: String) -> Self {
        Self {
            output,
            manifest,
            entries: Vec::new(),
        }
    }

    pub(super) fn open(&self) {
        if matches!(self.output, OutputKind::Human) {
            println!("Release checksums");
        }
    }

    pub(super) fn finish(&self, manifest: &str) {
        if matches!(self.output, OutputKind::Human) {
            println!("manifest  {manifest}");
        }
    }
}

impl toven_runtime::Progress<ChecksumOutcome> for ChecksumProjector {
    fn started(&mut self, unit_id: &str) -> AppResult<()> {
        if matches!(self.output, OutputKind::Human) {
            eprintln!("checksumming {unit_id}");
        }
        Ok(())
    }

    fn settled(&mut self, report: &toven_runtime::UnitReport<ChecksumOutcome>) -> AppResult<()> {
        let Some(entry) = &report.outcome else {
            return Ok(());
        };
        match self.output {
            OutputKind::Jsonl => {
                let record = ChecksumRecord {
                    manifest: self.manifest.clone(),
                    name: entry.name.clone(),
                    sha256: entry.sha256.clone(),
                    bytes: entry.bytes,
                };
                let line = serde_json::to_string(&record).map_err(AppError::internal)?;
                println!("{line}");
            }
            OutputKind::Human => println!("{}  {}  {}", entry.name, entry.sha256, entry.bytes),
        }
        self.entries.push(entry.clone());
        Ok(())
    }
}

/// A stable JSON-lines record for the signing outputs.
#[derive(Serialize)]
struct SignRecord {
    blob: String,
    bundle: String,
    backing: String,
}

/// Streams `release sign` as its single signature settles on the runtime
/// engine, never buffering a terminal aggregate before first output.
///
/// Human mode prints a `Release signing` header once, then one row as the
/// signature settles; a `signing` line goes to stderr for live motion. JSONL
/// mode emits one [`SignRecord`] to stdout.
pub(super) struct SignProjector {
    pub(super) output: OutputKind,
}

impl SignProjector {
    pub(super) const fn new(output: OutputKind) -> Self {
        Self { output }
    }

    pub(super) fn open(&self) {
        if matches!(self.output, OutputKind::Human) {
            println!("Release signing");
        }
    }
}

impl toven_runtime::Progress<SignOutcome> for SignProjector {
    fn started(&mut self, unit_id: &str) -> AppResult<()> {
        if matches!(self.output, OutputKind::Human) {
            eprintln!("signing {unit_id}");
        }
        Ok(())
    }

    fn settled(&mut self, report: &toven_runtime::UnitReport<SignOutcome>) -> AppResult<()> {
        let Some(sign) = &report.outcome else {
            return Ok(());
        };
        match self.output {
            OutputKind::Jsonl => {
                let record = SignRecord {
                    blob: sign.blob.clone(),
                    bundle: sign.bundle.clone(),
                    backing: sign.backing.to_string(),
                };
                let line = serde_json::to_string(&record).map_err(AppError::internal)?;
                println!("{line}");
            }
            OutputKind::Human => println!("{}  {}  {}", sign.blob, sign.bundle, sign.backing),
        }
        Ok(())
    }
}

/// A stable JSON-lines record for one archive's verification outcome.
#[derive(Serialize)]
struct VerifiedAssetRecord {
    mode: String,
    tag: Option<String>,
    expected_version: String,
    name: String,
    checksum_ok: Option<bool>,
    signature_ok: Option<bool>,
    ran: bool,
    reported_version: Option<String>,
}

/// Streams `release verify` per declared archive as each settles on the runtime
/// engine, never buffering a terminal aggregate before first output.
///
/// Human mode prints a `Release verify (<mode>, expected <version>)` header
/// once, then one row per archive to stdout as it settles; a `verifying` line
/// goes to stderr for live motion. JSONL mode emits one [`VerifiedAssetRecord`]
/// per archive to stdout.
pub(super) struct VerifyProjector {
    pub(super) output: OutputKind,
    pub(super) mode: String,
    pub(super) tag: Option<String>,
    pub(super) expected_version: String,
}

impl VerifyProjector {
    pub(super) const fn new(
        output: OutputKind,
        mode: String,
        tag: Option<String>,
        expected_version: String,
    ) -> Self {
        Self {
            output,
            mode,
            tag,
            expected_version,
        }
    }

    pub(super) fn open(&self) {
        if matches!(self.output, OutputKind::Human) {
            println!(
                "Release verify ({}, expected {})",
                self.mode, self.expected_version
            );
        }
    }
}

impl toven_runtime::Progress<VerifyOutcome> for VerifyProjector {
    fn started(&mut self, unit_id: &str) -> AppResult<()> {
        if matches!(self.output, OutputKind::Human) {
            eprintln!("verifying {unit_id}");
        }
        Ok(())
    }

    fn settled(&mut self, report: &toven_runtime::UnitReport<VerifyOutcome>) -> AppResult<()> {
        let Some(asset) = &report.outcome else {
            return Ok(());
        };
        match self.output {
            OutputKind::Jsonl => {
                let record = VerifiedAssetRecord {
                    mode: self.mode.clone(),
                    tag: self.tag.clone(),
                    expected_version: self.expected_version.clone(),
                    name: asset.name.clone(),
                    checksum_ok: asset.checksum_ok,
                    signature_ok: asset.signature_ok,
                    ran: asset.ran,
                    reported_version: asset.reported_version.clone(),
                };
                let line = serde_json::to_string(&record).map_err(AppError::internal)?;
                println!("{line}");
            }
            OutputKind::Human => println!(
                "{}  {}  {}  {}  {}",
                asset.name,
                render_optional_check(asset.checksum_ok),
                render_optional_check(asset.signature_ok),
                if asset.ran { "yes" } else { "no" },
                asset
                    .reported_version
                    .clone()
                    .unwrap_or_else(|| "—".to_string()),
            ),
        }
        Ok(())
    }
}

/// A stable JSON-lines record for one module's `release image` outcome.
#[derive(Serialize)]
struct ImageRecord {
    preview: bool,
    module: String,
    references: Vec<String>,
    digest: Option<String>,
    signed: bool,
    status: String,
}

/// Streams `release image` per module as each settles on the runtime engine,
/// never buffering a terminal aggregate before first output.
///
/// Human mode prints a `Release image[ (preview)]` header once, then one row
/// per module as it settles; a `building image for <module>` line goes to
/// stderr for live motion. JSONL mode emits one [`ImageRecord`] per module.
pub(super) struct ImageProjector {
    pub(super) output: OutputKind,
    pub(super) preview: bool,
}

impl ImageProjector {
    pub(super) const fn new(output: OutputKind, preview: bool) -> Self {
        Self { output, preview }
    }

    pub(super) fn open(&self) {
        if matches!(self.output, OutputKind::Human) {
            if self.preview {
                println!("Release image (preview)");
            } else {
                println!("Release image");
            }
        }
    }
}

impl toven_runtime::Progress<ImageModuleOutcome> for ImageProjector {
    fn started(&mut self, unit_id: &str) -> AppResult<()> {
        if matches!(self.output, OutputKind::Human) {
            eprintln!("building image for {unit_id}");
        }
        Ok(())
    }

    fn settled(&mut self, report: &toven_runtime::UnitReport<ImageModuleOutcome>) -> AppResult<()> {
        let Some(outcome) = &report.outcome else {
            return Ok(());
        };
        match self.output {
            OutputKind::Jsonl => {
                let record = ImageRecord {
                    preview: self.preview,
                    module: outcome.module.clone(),
                    references: outcome.references.clone(),
                    digest: outcome.digest.clone(),
                    signed: outcome.signed,
                    status: outcome.status.as_str().to_string(),
                };
                let line = serde_json::to_string(&record).map_err(AppError::internal)?;
                println!("{line}");
            }
            OutputKind::Human => println!(
                "{}  {}  {}  {}  {}",
                outcome.module,
                outcome.references.join(", "),
                outcome.digest.clone().unwrap_or_else(|| "—".to_string()),
                if outcome.signed { "yes" } else { "no" },
                outcome.status.as_str(),
            ),
        }
        Ok(())
    }
}

/// A stable JSON-lines record for one `release provenance` subject.
#[derive(Serialize)]
struct ProvenanceRecord {
    preview: bool,
    status: String,
    name: String,
    digest: String,
}

/// Streams `release provenance` per published subject as each attestation check
/// settles on the runtime engine, never buffering a terminal aggregate before
/// first output.
///
/// Human mode prints a `Release provenance[ (preview)]` header once, then one
/// row per subject as it settles; a `checking` line goes to stderr for live
/// motion. JSONL mode emits one [`ProvenanceRecord`] per subject.
pub(super) struct ProvenanceProjector {
    pub(super) output: OutputKind,
    pub(super) preview: bool,
}

impl ProvenanceProjector {
    pub(super) const fn new(output: OutputKind, preview: bool) -> Self {
        Self { output, preview }
    }

    pub(super) fn open(&self) {
        if matches!(self.output, OutputKind::Human) {
            if self.preview {
                println!("Release provenance (preview)");
            } else {
                println!("Release provenance");
            }
        }
    }
}

impl toven_runtime::Progress<ProvenanceOutcome> for ProvenanceProjector {
    fn started(&mut self, unit_id: &str) -> AppResult<()> {
        if matches!(self.output, OutputKind::Human) {
            eprintln!("checking {unit_id}");
        }
        Ok(())
    }

    fn settled(&mut self, report: &toven_runtime::UnitReport<ProvenanceOutcome>) -> AppResult<()> {
        let Some(entry) = &report.outcome else {
            return Ok(());
        };
        match self.output {
            OutputKind::Jsonl => {
                let record = ProvenanceRecord {
                    preview: self.preview,
                    status: entry.status.as_str().to_string(),
                    name: entry.subject.name.clone(),
                    digest: entry.subject.digest.clone(),
                };
                let line = serde_json::to_string(&record).map_err(AppError::internal)?;
                println!("{line}");
            }
            OutputKind::Human => println!(
                "{}  {}  {}",
                entry.subject.name,
                entry.subject.digest,
                entry.status.as_str()
            ),
        }
        Ok(())
    }
}
