//! `release checksums` verb: emit the `SHA256SUMS` manifest over the declared
//! release assets in `shasum -a 256` format.
//!
//! The engine owns release *policy* — which declared assets are checksum
//! *inputs* (every archive and the SBOM), which declared asset is the manifest
//! *output* (`SHA256SUMS`), and the exact on-disk format the downstream signer
//! (§05) and verifier (§06) round-trip — while [`rskit_util::hash::sha256`]
//! owns the digest algorithm and [`rskit_fs`] owns reading the bytes. The verb
//! is non-mutating: it never bumps a manifest, tags, or publishes.
//!
//! Inputs are the declared `[…release.host].assets` whose file name is neither
//! the `SHA256SUMS` manifest nor one of its signature sidecars
//! (`SHA256SUMS.*`), digested in declared order. The manifest is written to the
//! declared `SHA256SUMS` asset path. Every failure is fail closed with a typed
//! error: no declared assets, no `SHA256SUMS` manifest asset declared, an empty
//! input set, or a missing input file.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::safe_join;
use rskit_fs::sync_io::dir::create_all;
use rskit_fs::sync_io::file::{read as read_file, write_atomic};
use rskit_util::hash::sha256::sha256;
use tokio_util::sync::CancellationToken;
use toven_ports::{Provider, Reporter};
use toven_runtime::{Completed, UnitOperation, UnitSpec};

use crate::planning::plan::{release_targets, resolve_release_settings};
use toven_core::config::Document;
use toven_core::federation::resolve::PathDriverLocator;
use toven_core::plan::{PlanRequest, prepare_front};

/// The canonical file name of the checksum manifest and its signature sidecars'
/// stem. An asset whose file name is exactly this is the manifest output; one
/// beginning `SHA256SUMS.` is a signature sidecar (§05), not a checksum input.
const MANIFEST_NAME: &str = "SHA256SUMS";

/// One declared asset digested into the manifest.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ChecksumEntry {
    /// The manifest file name (the asset's final path component).
    pub name: String,
    /// The lowercase-hex SHA-256 digest of the asset's bytes.
    pub sha256: String,
    /// The asset's size in bytes.
    pub bytes: u64,
}

/// A read-only projection of the checksum manifest produced for the release
/// scope.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ChecksumReport {
    /// The project-relative path the `SHA256SUMS` manifest was written to.
    pub manifest: String,
    /// The per-asset checksum entries, in the manifest's line order.
    pub entries: Vec<ChecksumEntry>,
}

impl ChecksumReport {
    /// Construct a checksum report.
    #[must_use]
    pub const fn new(manifest: String, entries: Vec<ChecksumEntry>) -> Self {
        Self { manifest, entries }
    }

    /// Render the manifest body in `shasum -a 256` format: one
    /// `"<hex>  <name>\n"` line per entry (two-space separator, text mode).
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        for entry in &self.entries {
            out.push_str(&entry.sha256);
            out.push_str("  ");
            out.push_str(&entry.name);
            out.push('\n');
        }
        out
    }
}

/// Emit the `SHA256SUMS` manifest over the declared release assets.
///
/// # Errors
/// Fails closed with a typed error when no hosted-release assets are declared,
/// no `SHA256SUMS` manifest asset is declared, the checksum-input set is empty,
/// or an input asset file is missing — as well as propagating configuration,
/// discovery, graph, and I/O failures.
pub fn release_checksums(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    readers: &toven_core::federation::baseline::MemberVcsReaders<'_>,
    reporter: &mut dyn Reporter,
) -> AppResult<ChecksumReport> {
    let inputs = ChecksumInputs::gather(request, document, providers, readers, reporter)?;
    let entries = inputs
        .assets
        .iter()
        .map(|asset| checksum_for(&inputs, asset))
        .collect::<AppResult<Vec<_>>>()?;
    inputs.write_manifest(&entries)
}

/// Shared prerequisites for `release checksums`, resolved once before streaming.
pub struct ChecksumInputs {
    project_root: PathBuf,
    manifest: String,
    assets: Vec<String>,
}

impl ChecksumInputs {
    /// Resolve the declared checksum inputs and manifest path.
    ///
    /// # Errors
    /// Propagates configuration/discovery/graph failures and fail-closed asset validation errors.
    pub fn gather(
        request: &PlanRequest,
        document: &Document,
        providers: &[&dyn Provider],
        readers: &toven_core::federation::baseline::MemberVcsReaders<'_>,
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

        let declared = crate::artifacts::assets::declared_release_assets(&settings);
        if declared.is_empty() {
            return Err(AppError::invalid_input(
                "release.host.assets",
                "no hosted-release assets are declared; nothing to checksum (set \
                 […release.host] forge + assets)",
            ));
        }

        let manifest = declared
            .iter()
            .find(|asset| asset_file_name(asset) == Some(MANIFEST_NAME))
            .copied()
            .ok_or_else(|| {
                AppError::invalid_input(
                    "release.host.assets",
                    format!(
                        "no '{MANIFEST_NAME}' asset is declared to write the checksum manifest to"
                    ),
                )
            })?;

        let assets: Vec<String> = declared
            .iter()
            .filter(|asset| is_checksum_input(asset))
            .map(|asset| (*asset).clone())
            .collect();
        if assets.is_empty() {
            return Err(AppError::invalid_input(
                "release.host.assets",
                "no checksum-input assets are declared (every declared asset is the \
                 manifest or one of its signature sidecars)",
            ));
        }

        Ok(Self {
            project_root: request.project_root.as_path().to_path_buf(),
            manifest: manifest.clone(),
            assets,
        })
    }

    fn asset(&self, id: &str) -> Option<&String> {
        self.assets.iter().find(|asset| asset.as_str() == id)
    }

    /// The engine unit graph: one independent unit per checksum input asset.
    #[must_use]
    pub fn units(&self) -> Vec<UnitSpec> {
        self.assets
            .iter()
            .map(|asset| UnitSpec::new(asset.clone(), Vec::<String>::new()))
            .collect()
    }

    /// The project-relative checksum manifest path.
    #[must_use]
    pub fn manifest(&self) -> &str {
        &self.manifest
    }

    /// Write the final `SHA256SUMS` manifest from streamed entries.
    ///
    /// # Errors
    /// Propagates manifest path and write failures.
    pub fn write_manifest(&self, entries: &[ChecksumEntry]) -> AppResult<ChecksumReport> {
        let report = ChecksumReport::new(self.manifest.clone(), entries.to_vec());
        let dest = safe_join(&self.project_root, &self.manifest).map_err(|error| {
            AppError::invalid_input(
                "release.host.assets",
                format!(
                    "asset '{}' is not a safe project-relative path",
                    self.manifest
                ),
            )
            .with_cause(error)
        })?;
        if let Some(parent) = dest.parent() {
            create_all(parent)?;
        }
        write_atomic(&dest, report.render(), "toven-sha256sums")?;
        Ok(report)
    }
}

/// One settled checksum entry.
pub type ChecksumOutcome = ChecksumEntry;

fn checksum_for(inputs: &ChecksumInputs, asset: &str) -> AppResult<ChecksumOutcome> {
    checksum_entry(&inputs.project_root, asset)
}

/// The `release checksums` per-unit operation on the shared runtime engine.
pub struct ChecksumOperation {
    inputs: Arc<ChecksumInputs>,
}

impl ChecksumOperation {
    /// Wrap gathered inputs as a runnable operation.
    #[must_use]
    pub fn new(inputs: ChecksumInputs) -> Self {
        Self {
            inputs: Arc::new(inputs),
        }
    }

    /// Share the gathered inputs so the CLI can write the manifest after streaming.
    #[must_use]
    pub fn inputs(&self) -> Arc<ChecksumInputs> {
        Arc::clone(&self.inputs)
    }
}

#[async_trait]
impl UnitOperation for ChecksumOperation {
    type Shared = Arc<ChecksumInputs>;
    type Outcome = ChecksumOutcome;

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
        let id = unit_id.to_string();
        let outcome = tokio::task::spawn_blocking(move || {
            let asset = shared.asset(&id).ok_or_else(|| {
                AppError::new(
                    ErrorCode::Internal,
                    format!("unknown checksums unit '{id}'"),
                )
            })?;
            checksum_for(&shared, asset)
        })
        .await
        .map_err(AppError::internal)??;
        Ok(Completed::succeeded(outcome))
    }
}

/// Build the `release checksums` operation and its engine unit graph.
///
/// # Errors
/// Propagates GATHER failures.
pub fn checksums_operation(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    readers: &toven_core::federation::baseline::MemberVcsReaders<'_>,
    reporter: &mut dyn Reporter,
) -> AppResult<(ChecksumOperation, Vec<UnitSpec>)> {
    let inputs = ChecksumInputs::gather(request, document, providers, readers, reporter)?;
    let units = inputs.units();
    Ok((ChecksumOperation::new(inputs), units))
}

/// Digest one declared asset, reading its bytes through the filesystem owner.
fn checksum_entry(project_root: &Path, asset: &str) -> AppResult<ChecksumEntry> {
    let name = asset_file_name(asset)
        .ok_or_else(|| {
            AppError::invalid_input(
                "release.host.assets",
                format!("asset '{asset}' has no file name to checksum"),
            )
        })?
        .to_string();
    let path = safe_join(project_root, asset).map_err(|error| {
        AppError::invalid_input(
            "release.host.assets",
            format!("asset '{asset}' is not a safe project-relative path"),
        )
        .with_cause(error)
    })?;
    let bytes = read_file(&path).map_err(|error| {
        AppError::invalid_input(
            "release.checksums.input",
            format!("declared checksum input '{asset}' is missing; produce it before checksumming"),
        )
        .with_cause(error)
    })?;
    let digest = sha256(&bytes);
    Ok(ChecksumEntry {
        name,
        sha256: digest.to_hex(),
        bytes: bytes.len() as u64,
    })
}

/// True when a declared asset is a checksum *input* — neither the `SHA256SUMS`
/// manifest nor one of its `SHA256SUMS.*` signature sidecars.
fn is_checksum_input(asset: &str) -> bool {
    asset_file_name(asset).is_some_and(|name| {
        name != MANIFEST_NAME && !name.starts_with(&format!("{MANIFEST_NAME}."))
    })
}

/// The final path component of a project-relative asset path.
fn asset_file_name(asset: &str) -> Option<&str> {
    Path::new(asset).file_name().and_then(|name| name.to_str())
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
        BaselineSpec, CommonEcosystemConfig, DiscoverResponse, HostConfig, Provider, ReleaseConfig,
        TaskIntent,
    };
    use toven_testkit::{
        FakeConfiguredAdapter, FakeProvider, FakeReleaseTarget, FakeVcsReader, RecordingReporter,
    };

    use super::{ChecksumOutcome, ChecksumReport, checksums_operation, release_checksums};
    use toven_core::config::{Document, ProjectConfig, TovenConfig};
    use toven_core::federation::MemberVcsReaders;
    use toven_core::plan::PlanRequest;

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
            hooks: std::collections::BTreeMap::new(),
            units: std::collections::BTreeMap::new(),
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

    fn write_asset(root: &Path, rel: &str, bytes: &[u8]) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    /// Parse a `SHA256SUMS` body into `(name, hex)` pairs, mirroring
    /// `shasum -a 256 -c` line grammar (64 hex chars, two spaces, name).
    fn parse(body: &str) -> Vec<(String, String)> {
        body.lines()
            .map(|line| {
                let (hex, name) = line.split_once("  ").expect("two-space separator");
                assert_eq!(hex.len(), 64, "digest must be 64 hex chars");
                (name.to_string(), hex.to_string())
            })
            .collect()
    }

    #[test]
    fn emits_the_manifest_over_declared_inputs_in_declared_order() {
        let root = TempDir::new().unwrap();
        write_asset(
            root.path(),
            "dist/toven-x86_64-apple-darwin.tar.gz",
            b"archive-a",
        );
        write_asset(root.path(), "dist/toven-sbom.cdx.json", b"sbom");
        let provider = provider_with_assets(vec![
            "dist/toven-x86_64-apple-darwin.tar.gz",
            "dist/SHA256SUMS",
            "dist/SHA256SUMS.bundle",
            "dist/toven-sbom.cdx.json",
        ]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let reader = FakeVcsReader::new();
        let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let report = release_checksums(
            &request(root.path()),
            &document(),
            &providers,
            &readers,
            &mut reporter,
        )
        .unwrap();

        // The SHA256SUMS manifest and its signature sidecars are excluded; the
        // archive and the SBOM are digested in their declared order.
        assert_eq!(report.manifest, "dist/SHA256SUMS");
        let names: Vec<&str> = report.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["toven-x86_64-apple-darwin.tar.gz", "toven-sbom.cdx.json"]
        );

        // The manifest was written to its declared asset path with matching body.
        let written = std::fs::read_to_string(root.path().join("dist").join("SHA256SUMS")).unwrap();
        assert_eq!(written, report.render());
    }

    #[test]
    fn emitted_manifest_verifies_under_check_semantics() {
        let root = TempDir::new().unwrap();
        write_asset(
            root.path(),
            "dist/toven-x86_64-apple-darwin.tar.gz",
            b"the-archive-bytes",
        );
        write_asset(root.path(), "dist/toven-sbom.cdx.json", b"the-sbom-bytes");
        let provider = provider_with_assets(vec![
            "dist/toven-x86_64-apple-darwin.tar.gz",
            "dist/SHA256SUMS",
            "dist/toven-sbom.cdx.json",
        ]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let reader = FakeVcsReader::new();
        let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let report = release_checksums(
            &request(root.path()),
            &document(),
            &providers,
            &readers,
            &mut reporter,
        )
        .unwrap();

        // Re-digest each listed asset from disk and compare — this is exactly
        // `shasum -a 256 -c SHA256SUMS` executed from `dist/`.
        for (name, expected) in parse(&report.render()) {
            let bytes = std::fs::read(root.path().join("dist").join(&name)).unwrap();
            let actual = rskit_util::hash::sha256::sha256(&bytes).to_hex();
            assert_eq!(actual, expected, "checksum line for {name} must verify");
        }
    }

    #[test]
    fn manifest_render_uses_shasum_two_space_format() {
        let report = ChecksumReport::new(
            "dist/SHA256SUMS".to_string(),
            vec![super::ChecksumEntry {
                name: "a.tar.gz".to_string(),
                sha256: "abc".to_string(),
                bytes: 3,
            }],
        );
        assert_eq!(report.render(), "abc  a.tar.gz\n");
    }

    #[derive(Default)]
    struct Recorder {
        started: Vec<String>,
        settled: Vec<(String, toven_runtime::UnitStatus, Option<ChecksumOutcome>)>,
    }

    impl toven_runtime::Progress<ChecksumOutcome> for Recorder {
        fn started(&mut self, unit_id: &str) -> rskit_errors::AppResult<()> {
            self.started.push(unit_id.to_string());
            Ok(())
        }

        fn settled(
            &mut self,
            report: &toven_runtime::UnitReport<ChecksumOutcome>,
        ) -> rskit_errors::AppResult<()> {
            self.settled.push((
                report.unit_id.clone(),
                report.status,
                report.outcome.clone(),
            ));
            Ok(())
        }
    }

    #[test]
    fn checksums_units_are_edgeless_one_per_input_asset() {
        let root = TempDir::new().unwrap();
        write_asset(root.path(), "dist/a.tar.gz", b"a");
        write_asset(root.path(), "dist/b.cdx.json", b"b");
        let provider =
            provider_with_assets(vec!["dist/a.tar.gz", "dist/SHA256SUMS", "dist/b.cdx.json"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let reader = FakeVcsReader::new();
        let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let (_op, units) = checksums_operation(
            &request(root.path()),
            &document(),
            &providers,
            &readers,
            &mut reporter,
        )
        .unwrap();

        assert_eq!(units.len(), 2);
        assert!(units.iter().all(|unit| unit.depends_on.is_empty()));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn checksums_streams_a_settled_entry_per_asset() {
        let root = TempDir::new().unwrap();
        write_asset(root.path(), "dist/a.tar.gz", b"a");
        write_asset(root.path(), "dist/b.cdx.json", b"b");
        let provider =
            provider_with_assets(vec!["dist/a.tar.gz", "dist/SHA256SUMS", "dist/b.cdx.json"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let reader = FakeVcsReader::new();
        let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let (op, units) = checksums_operation(
            &request(root.path()),
            &document(),
            &providers,
            &readers,
            &mut reporter,
        )
        .unwrap();
        let mut rec = Recorder::default();
        let summary = toven_runtime::execute(
            &units,
            op,
            toven_runtime::EngineConfig {
                jobs: 2,
                fail_fast: false,
            },
            &mut rec,
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();

        assert_eq!(summary.total, 2);
        assert_eq!(summary.succeeded, 2);
        assert_eq!(rec.started.len(), 2);
        assert_eq!(rec.settled.len(), 2);
        for (unit_id, status, outcome) in &rec.settled {
            assert_eq!(*status, toven_runtime::UnitStatus::Succeeded);
            assert_eq!(
                &outcome.as_ref().unwrap().name,
                Path::new(unit_id).file_name().unwrap().to_str().unwrap()
            );
        }
    }

    #[test]
    fn fails_closed_when_an_input_asset_is_missing() {
        let root = TempDir::new().unwrap();
        // Declare an archive input but never write it to disk.
        let provider = provider_with_assets(vec![
            "dist/toven-x86_64-apple-darwin.tar.gz",
            "dist/SHA256SUMS",
        ]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let reader = FakeVcsReader::new();
        let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let error = release_checksums(
            &request(root.path()),
            &document(),
            &providers,
            &readers,
            &mut reporter,
        )
        .expect_err("a missing checksum input must fail closed");
        assert!(
            error
                .to_string()
                .contains("toven-x86_64-apple-darwin.tar.gz")
        );
    }

    #[test]
    fn fails_closed_when_the_input_set_is_empty() {
        let root = TempDir::new().unwrap();
        // Only the manifest and its sidecars are declared — no inputs to digest.
        let provider = provider_with_assets(vec!["dist/SHA256SUMS", "dist/SHA256SUMS.bundle"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let reader = FakeVcsReader::new();
        let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let error = release_checksums(
            &request(root.path()),
            &document(),
            &providers,
            &readers,
            &mut reporter,
        )
        .expect_err("an empty checksum-input set must fail closed");
        assert!(error.to_string().contains("no checksum-input assets"));
    }

    #[test]
    fn fails_closed_when_no_manifest_asset_is_declared() {
        let root = TempDir::new().unwrap();
        write_asset(root.path(), "dist/toven-x86_64-apple-darwin.tar.gz", b"a");
        let provider = provider_with_assets(vec!["dist/toven-x86_64-apple-darwin.tar.gz"]);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let reader = FakeVcsReader::new();
        let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let error = release_checksums(
            &request(root.path()),
            &document(),
            &providers,
            &readers,
            &mut reporter,
        )
        .expect_err("a missing SHA256SUMS asset must fail closed");
        assert!(error.to_string().contains("SHA256SUMS"));
    }
}
