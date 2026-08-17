//! Read-only `release sbom` projection.
//!
//! Orchestrates each releasable module's ecosystem SBOM tool argv-first via its
//! [`SbomProducer`](toven_ports::SbomProducer), collecting the produced
//! `CycloneDX` artifacts into a bounded output directory. Toven owns scope,
//! ordering, and reporting; the ecosystem target owns the tool invocation. A
//! module whose ecosystem has no SBOM tooling is recorded as skipped rather
//! than failing the whole projection.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::safe_join;
use rskit_fs::sync_io::dir::create_all;
use rskit_fs::sync_io::file::copy as copy_file;
use tokio_util::sync::CancellationToken;
use toven_model::{EcosystemId, MemberId, Module, ModuleKey};
use toven_ports::{Provider, PublicationPolicy, Reporter};
use toven_runtime::{Completed, UnitOperation, UnitSpec};

use crate::ArtifactManifest;
use crate::ReleaseTargets;
use crate::ResolvedReleaseSettings;
use crate::planning::plan::{release_targets, resolve_release_settings};
use toven_core::config::Document;
use toven_core::federation::baseline::MemberVcsReaders;
use toven_core::federation::resolve::PathDriverLocator;
use toven_core::plan::{PlanRequest, prepare_front};

/// The `CycloneDX` file-name extension every SBOM artifact and asset carries.
const SBOM_EXTENSION: &str = ".cdx.json";

/// One declared SBOM asset the projection staged from a produced artifact.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct StagedSbom {
    /// The project-relative asset path the SBOM was written to.
    pub asset: String,
    /// The module label whose SBOM artifact was staged.
    pub source: String,
}

/// A read-only projection of the SBOM artifacts produced for the release scope.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SbomReport {
    /// The `CycloneDX` artifacts written into the bounded output directory.
    pub artifacts: Vec<ArtifactManifest>,
    /// Modules whose ecosystem has no SBOM tooling (skipped, not failed).
    pub skipped: Vec<ModuleKey>,
    /// Declared hosted-release SBOM assets staged from a produced artifact.
    pub staged: Vec<StagedSbom>,
}

impl SbomReport {
    /// Construct an SBOM report with no staged assets.
    #[must_use]
    pub const fn new(artifacts: Vec<ArtifactManifest>, skipped: Vec<ModuleKey>) -> Self {
        Self {
            artifacts,
            skipped,
            staged: Vec::new(),
        }
    }
}

/// Generate a `CycloneDX` SBOM per releasable module under `out_dir`.
///
/// A module is releasable when its ecosystem adapter exposes a release target
/// and it is not release-excluded. Each target's SBOM tool is invoked
/// argv-first, bounded to `out_dir`; a target with no SBOM tooling contributes
/// a skip. The output directory is created if missing; nothing outside it is
/// touched.
///
/// # Errors
/// Propagates configuration/discovery/graph failures, output-directory I/O
/// failures, and SBOM tool spawn/exit failures.
pub fn release_sbom(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    readers: &MemberVcsReaders<'_>,
    out_dir: &Path,
    reporter: &mut dyn Reporter,
) -> AppResult<SbomReport> {
    let inputs = SbomInputs::gather(request, document, providers, readers, out_dir, reporter)?;
    let mut artifacts = Vec::new();
    let mut skipped = Vec::new();
    for module in &inputs.modules {
        match sbom_for(&inputs, module)?.artifact {
            Some(artifact) => artifacts.push(artifact),
            None => skipped.push(module.key.clone()),
        }
    }
    artifacts.sort_by(|left, right| left.label.cmp(&right.label));
    skipped.sort();
    let staged = inputs.stage(&artifacts)?;
    Ok(SbomReport {
        artifacts,
        skipped,
        staged,
    })
}

/// One releasable module's fully-owned SBOM inputs, resolved during GATHER so
/// the streamed per-unit phase borrows neither the providers nor VCS readers.
struct SbomModule {
    /// Stable unit id (the module's canonical key string).
    id: String,
    /// The module's canonical key.
    key: ModuleKey,
    /// The train the module's release target is keyed under.
    member: Option<MemberId>,
    /// The train's ecosystem.
    ecosystem: EcosystemId,
    /// The discovered module handed to the SBOM tool.
    module: Module,
}

/// The shared prerequisites for `release sbom`, resolved once by
/// [`SbomInputs::gather`] and shared across every per-unit run.
pub struct SbomInputs {
    /// Release targets keyed by `(member, ecosystem)` — `Send + Sync` so they
    /// cross the engine's worker pool.
    targets: ReleaseTargets,
    /// Each module's resolved release settings, keyed by module key.
    settings: std::collections::BTreeMap<ModuleKey, ResolvedReleaseSettings>,
    /// The bounded output directory every SBOM artifact is written under.
    out_dir: PathBuf,
    /// The project root the declared hosted assets are staged relative to.
    project_root: PathBuf,
    /// The releasable modules, in discovery order.
    modules: Vec<SbomModule>,
}

impl SbomInputs {
    /// Resolve the release targets, settings, and releasable-module set once,
    /// creating the bounded output directory.
    ///
    /// A module is releasable when its ecosystem adapter exposes a release
    /// target and it is not release-excluded. A module with a release target but
    /// no resolved settings is an internal invariant violation and fails closed.
    ///
    /// # Errors
    /// Propagates configuration/discovery/graph failures, output-directory I/O
    /// failures, and an unresolved-settings invariant violation.
    pub fn gather(
        request: &PlanRequest,
        document: &Document,
        providers: &[&dyn Provider],
        readers: &MemberVcsReaders<'_>,
        out_dir: &Path,
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
        create_all(out_dir)?;

        let mut modules = Vec::new();
        for module in &context.federation.modules {
            let key = (module.member.clone(), module.id.ecosystem.clone());
            if !targets.contains_key(&key) {
                continue;
            }
            // A release-excluded module is outside the release surface (no bump,
            // tag, publish, or hosted-release asset), so it contributes no SBOM.
            // Every module with a release target has resolved settings, so a
            // missing entry is an internal invariant violation: fail closed
            // rather than silently emit an SBOM for a module whose release scope
            // is unknown.
            let resolved = settings.get(&module.key()).ok_or_else(|| {
                AppError::new(
                    ErrorCode::Internal,
                    format!(
                        "module '{}' has a release target but no resolved release settings",
                        module.key()
                    ),
                )
            })?;
            if matches!(resolved.publication, PublicationPolicy::Excluded) {
                continue;
            }
            modules.push(SbomModule {
                id: module.key().to_string(),
                key: module.key(),
                member: module.member.clone(),
                ecosystem: module.id.ecosystem.clone(),
                module: module.clone(),
            });
        }
        Ok(Self {
            targets,
            settings,
            out_dir: out_dir.to_path_buf(),
            project_root: request.project_root.as_path().to_path_buf(),
            modules,
        })
    }

    /// Look up a releasable module by its unit id.
    fn module(&self, id: &str) -> Option<&SbomModule> {
        self.modules.iter().find(|module| module.id == id)
    }

    /// The engine unit graph: one unit per releasable module, serialized within
    /// each shared release target and parallel across independent targets.
    ///
    /// Modules that share a `(member, ecosystem)` release target share one
    /// ecosystem workspace, and an SBOM tool can resolve that whole workspace and
    /// write output beside *every* member manifest (`cargo cyclonedx` does), so
    /// two such modules run concurrently would race on those sibling files — one
    /// unit's stray-cleanup could delete another's output mid-run. Chaining the
    /// modules of each target into a serial dependency line makes them run one at
    /// a time, while modules of independent targets stay edgeless and run as one
    /// bounded-parallel wave.
    #[must_use]
    pub fn units(&self) -> Vec<UnitSpec> {
        let mut previous: std::collections::BTreeMap<(Option<MemberId>, EcosystemId), String> =
            std::collections::BTreeMap::new();
        let mut units = Vec::with_capacity(self.modules.len());
        for module in &self.modules {
            let target = (module.member.clone(), module.ecosystem.clone());
            let depends_on = previous
                .get(&target)
                .cloned()
                .into_iter()
                .collect::<Vec<_>>();
            units.push(UnitSpec::new(module.id.clone(), depends_on));
            previous.insert(target, module.id.clone());
        }
        units
    }

    /// Stage every declared SBOM hosted-release asset from the produced
    /// artifacts — the post-stream aggregate assembled from the streamed
    /// outcomes.
    ///
    /// # Errors
    /// Propagates staging I/O failures and a declared-asset-without-artifact
    /// fail-closed violation.
    pub fn stage(&self, artifacts: &[ArtifactManifest]) -> AppResult<Vec<StagedSbom>> {
        stage_sbom_assets(&self.project_root, &self.settings, artifacts)
    }
}

/// One module's settled SBOM outcome: the produced artifact, or `None` when the
/// ecosystem has no SBOM tooling (a skip, not a failure).
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SbomOutcome {
    /// The module the outcome is for.
    pub module: ModuleKey,
    /// The produced `CycloneDX` artifact, or `None` when the module was skipped.
    pub artifact: Option<ArtifactManifest>,
}

/// Produce one releasable module's SBOM — the pure per-unit compute over the
/// gathered [`SbomInputs`], with the argv-first tool invocation as its only I/O.
///
/// # Errors
/// Returns [`ErrorCode::Internal`] if the module's release target is missing
/// from the gathered set, and propagates SBOM tool spawn/exit failures.
fn sbom_for(inputs: &SbomInputs, module: &SbomModule) -> AppResult<SbomOutcome> {
    let target = inputs
        .targets
        .get(&(module.member.clone(), module.ecosystem.clone()))
        .ok_or_else(|| {
            AppError::new(
                ErrorCode::Internal,
                format!("module '{}' has no gathered release target", module.key),
            )
        })?;
    let artifact = target
        .sbom(&module.module, &inputs.out_dir)?
        .map(|artifact| ArtifactManifest::new(module.key.to_string(), artifact.path));
    Ok(SbomOutcome {
        module: module.key.clone(),
        artifact,
    })
}

/// The `release sbom` per-unit operation on the shared runtime engine.
///
/// GATHER resolves the targets, settings, and output directory once into
/// [`SbomInputs`]; each unit streams one module's SBOM tool invocation. That
/// invocation is a synchronous port call, so it runs on a blocking thread
/// ([`tokio::task::spawn_blocking`]) to let the engine schedule the modules
/// bounded-parallel.
pub struct SbomOperation {
    inputs: Arc<SbomInputs>,
}

impl SbomOperation {
    /// Wrap gathered inputs as a runnable operation.
    #[must_use]
    pub fn new(inputs: SbomInputs) -> Self {
        Self {
            inputs: Arc::new(inputs),
        }
    }

    /// Share the gathered inputs so the CLI can stage declared assets from the
    /// streamed artifacts once the run completes.
    #[must_use]
    pub fn inputs(&self) -> Arc<SbomInputs> {
        Arc::clone(&self.inputs)
    }
}

#[async_trait]
impl UnitOperation for SbomOperation {
    type Shared = Arc<SbomInputs>;
    type Outcome = SbomOutcome;

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
            let module = shared.module(&id).ok_or_else(|| {
                AppError::new(ErrorCode::Internal, format!("unknown sbom unit '{id}'"))
            })?;
            sbom_for(&shared, module)
        })
        .await
        .map_err(AppError::internal)??;
        Ok(Completed::succeeded(outcome))
    }
}

/// Build the `release sbom` operation and its engine unit graph.
///
/// # Errors
/// Propagates GATHER failures (configuration/discovery/graph, output-directory
/// I/O, unresolved settings).
pub fn sbom_operation(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    readers: &MemberVcsReaders<'_>,
    out_dir: &Path,
    reporter: &mut dyn Reporter,
) -> AppResult<(SbomOperation, Vec<UnitSpec>)> {
    let inputs = SbomInputs::gather(request, document, providers, readers, out_dir, reporter)?;
    let units = inputs.units();
    Ok((SbomOperation::new(inputs), units))
}

/// Stage every declared `*.cdx.json` hosted-release asset from the produced SBOM
/// artifact whose file stem matches the asset. Fail-closed when a declared SBOM
/// asset has no matching produced artifact so the release surface never ships a
/// missing SBOM.
fn stage_sbom_assets(
    project_root: &Path,
    settings: &std::collections::BTreeMap<ModuleKey, crate::ResolvedReleaseSettings>,
    artifacts: &[ArtifactManifest],
) -> AppResult<Vec<StagedSbom>> {
    let mut assets: Vec<&String> = settings
        .values()
        .flat_map(|resolved| resolved.host.assets.iter())
        .filter(|asset| asset.ends_with(SBOM_EXTENSION))
        .collect();
    assets.sort();
    assets.dedup();

    let mut staged = Vec::new();
    for asset in assets {
        let wanted = format!("{}{SBOM_EXTENSION}", sbom_asset_stem(asset));
        let artifact = artifacts
            .iter()
            .find(|artifact| {
                artifact.path.file_name().and_then(|name| name.to_str()) == Some(wanted.as_str())
            })
            .ok_or_else(|| {
                AppError::invalid_input(
                    "release.host.assets",
                    format!(
                        "declared SBOM asset '{asset}' has no produced artifact named '{wanted}'"
                    ),
                )
            })?;
        let dest = safe_join(project_root, asset).map_err(|error| {
            AppError::invalid_input(
                "release.host.assets",
                format!("asset '{asset}' is not a safe project-relative path"),
            )
            .with_cause(error)
        })?;
        if let Some(parent) = dest.parent() {
            create_all(parent)?;
        }
        copy_file(&artifact.path, &dest)?;
        staged.push(StagedSbom {
            asset: asset.clone(),
            source: artifact.label.clone(),
        });
    }
    staged.sort_by(|left, right| left.asset.cmp(&right.asset));
    Ok(staged)
}

/// Derive the produced-artifact stem for a declared SBOM asset: strip the
/// `.cdx.json` extension and an optional `-sbom` suffix so `toven-sbom.cdx.json`
/// and `toven.cdx.json` both map to the `toven.cdx.json` artifact.
fn sbom_asset_stem(asset: &str) -> &str {
    let file = asset.rsplit(['/', '\\']).next().unwrap_or(asset);
    let base = file.strip_suffix(SBOM_EXTENSION).unwrap_or(file);
    base.strip_suffix("-sbom").unwrap_or(base)
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

    use super::{SbomOutcome, release_sbom, sbom_operation};
    use toven_core::config::{Document, ModuleConfig, ProjectConfig, TovenConfig};
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

    /// A document whose per-module override marks `key` release-excluded.
    fn document_excluding(key: &str) -> Document {
        let mut document = document();
        let mut over = ModuleConfig::default();
        over.release.exclude = Some(true);
        document.modules.insert(key.to_string(), over);
        document
    }

    fn request() -> PlanRequest {
        PlanRequest::new(
            "r1",
            "demo",
            TaskIntent::resolve("release"),
            AbsPath::new("/repo").unwrap(),
        )
    }

    fn providers_with(target: FakeReleaseTarget) -> FakeProvider {
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![module("core")];
        let adapter = FakeConfiguredAdapter::new(eid("rust"))
            .with_response(response)
            .with_release_target(target);
        FakeProvider::new(eid("rust")).with_adapter(adapter)
    }

    /// Build a provider whose ecosystem declares `assets` on a `github` forge
    /// and whose target produces `artifact` as its SBOM file.
    fn providers_with_assets(assets: Vec<&str>, artifact: &str) -> FakeProvider {
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
            .with_release_target(FakeReleaseTarget::new().with_sbom_artifact(artifact))
            .with_common(common);
        FakeProvider::new(eid("rust")).with_adapter(adapter)
    }

    fn request_at(root: &Path) -> PlanRequest {
        PlanRequest::new(
            "r1",
            "demo",
            TaskIntent::resolve("release"),
            AbsPath::new(root.to_str().unwrap()).unwrap(),
        )
    }

    #[test]
    fn sbom_invokes_the_target_per_module_and_returns_bounded_paths() {
        let target = FakeReleaseTarget::new().with_sbom_artifact("core.cdx.json");
        let recorded = target.clone();
        let provider = providers_with(target);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let reader = FakeVcsReader::new();
        let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));

        let out = TempDir::new().unwrap();
        let mut reporter = RecordingReporter::new();

        let report = release_sbom(
            &request(),
            &document(),
            &providers,
            &readers,
            out.path(),
            &mut reporter,
        )
        .unwrap();

        assert_eq!(report.artifacts.len(), 1);
        assert!(report.skipped.is_empty());
        let artifact = &report.artifacts[0];
        assert_eq!(artifact.label, "rust:core");
        assert_eq!(artifact.path, out.path().join("core.cdx.json"));
        // The SBOM invocation was recorded against the bounded output directory.
        let calls = recorded.calls();
        assert!(calls.iter().any(|call| matches!(
            call,
            toven_testkit::ReleaseCall::Sbom { out_dir, .. }
                if out_dir == &out.path().display().to_string()
        )));
    }

    #[test]
    fn sbom_records_a_skip_when_the_ecosystem_has_no_tooling() {
        let target = FakeReleaseTarget::new().with_sbom_unsupported();
        let provider = providers_with(target);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let reader = FakeVcsReader::new();
        let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));

        let out = TempDir::new().unwrap();
        let mut reporter = RecordingReporter::new();

        let report = release_sbom(
            &request(),
            &document(),
            &providers,
            &readers,
            out.path(),
            &mut reporter,
        )
        .unwrap();

        assert!(report.artifacts.is_empty());
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0], module("core").key());
    }

    #[test]
    fn sbom_omits_a_release_excluded_module() {
        // A release-excluded module is not part of the release surface, so it
        // must contribute no SBOM artifact and is not a tooling skip either.
        let target = FakeReleaseTarget::new().with_sbom_artifact("core.cdx.json");
        let provider = providers_with(target);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let reader = FakeVcsReader::new();
        let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));

        let out = TempDir::new().unwrap();
        let mut reporter = RecordingReporter::new();

        let report = release_sbom(
            &request(),
            &document_excluding("rust:core"),
            &providers,
            &readers,
            out.path(),
            &mut reporter,
        )
        .unwrap();

        assert!(report.artifacts.is_empty());
        assert!(report.skipped.is_empty());
    }

    #[test]
    fn sbom_propagates_a_tool_failure() {
        let target = FakeReleaseTarget::new().with_sbom_failure("cyclonedx exploded");
        let provider = providers_with(target);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let reader = FakeVcsReader::new();
        let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));

        let out = TempDir::new().unwrap();
        let mut reporter = RecordingReporter::new();

        let error = release_sbom(
            &request(),
            &document(),
            &providers,
            &readers,
            out.path(),
            &mut reporter,
        )
        .expect_err("a tool failure must surface, not be swallowed");
        assert!(error.to_string().contains("cyclonedx exploded"));
    }

    #[test]
    fn sbom_stages_the_declared_asset_from_the_produced_artifact() {
        let root = TempDir::new().unwrap();
        let provider = providers_with_assets(vec!["dist/toven-sbom.cdx.json"], "toven.cdx.json");
        let providers: Vec<&dyn Provider> = vec![&provider];
        let reader = FakeVcsReader::new();
        let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));

        let out = TempDir::new().unwrap();
        let mut reporter = RecordingReporter::new();

        let report = release_sbom(
            &request_at(root.path()),
            &document(),
            &providers,
            &readers,
            out.path(),
            &mut reporter,
        )
        .unwrap();

        assert_eq!(report.staged.len(), 1);
        assert_eq!(report.staged[0].asset, "dist/toven-sbom.cdx.json");
        assert_eq!(report.staged[0].source, "rust:core");
        let staged = root.path().join("dist").join("toven-sbom.cdx.json");
        let produced = out.path().join("toven.cdx.json");
        assert_eq!(
            std::fs::read(&staged).unwrap(),
            std::fs::read(&produced).unwrap(),
            "the staged asset must be a byte-for-byte copy of the produced artifact",
        );
    }

    #[test]
    fn sbom_fails_closed_when_a_declared_asset_has_no_produced_artifact() {
        let root = TempDir::new().unwrap();
        // The declared asset maps to a `missing.cdx.json` stem, but the target
        // produces `toven.cdx.json`, so staging must fail rather than skip.
        let provider = providers_with_assets(vec!["dist/missing-sbom.cdx.json"], "toven.cdx.json");
        let providers: Vec<&dyn Provider> = vec![&provider];
        let reader = FakeVcsReader::new();
        let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));

        let out = TempDir::new().unwrap();
        let mut reporter = RecordingReporter::new();

        let error = release_sbom(
            &request_at(root.path()),
            &document(),
            &providers,
            &readers,
            out.path(),
            &mut reporter,
        )
        .expect_err("a declared SBOM asset with no artifact must fail closed");
        assert!(error.to_string().contains("missing.cdx.json"));
    }

    #[derive(Default)]
    struct Recorder {
        started: Vec<String>,
        settled: Vec<(String, toven_runtime::UnitStatus, Option<SbomOutcome>)>,
    }

    impl toven_runtime::Progress<SbomOutcome> for Recorder {
        fn started(&mut self, unit_id: &str) -> rskit_errors::AppResult<()> {
            self.started.push(unit_id.to_string());
            Ok(())
        }

        fn settled(
            &mut self,
            report: &toven_runtime::UnitReport<SbomOutcome>,
        ) -> rskit_errors::AppResult<()> {
            self.settled.push((
                report.unit_id.clone(),
                report.status,
                report.outcome.clone(),
            ));
            Ok(())
        }
    }

    /// A provider with two releasable modules whose target produces an SBOM.
    fn two_module_provider() -> FakeProvider {
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![module("core"), module("cli")];
        let adapter = FakeConfiguredAdapter::new(eid("rust"))
            .with_response(response)
            .with_release_target(FakeReleaseTarget::new().with_sbom_artifact("sbom.cdx.json"));
        FakeProvider::new(eid("rust")).with_adapter(adapter)
    }

    #[test]
    fn sbom_units_of_a_shared_target_are_serialized() {
        // `core` and `cli` share one `(member, ecosystem)` release target — one
        // Cargo workspace — so their SBOM tool would race on the sibling files
        // `cargo cyclonedx` writes beside every member manifest. The units must
        // therefore form a serial chain (the second depends on the first), not
        // an edgeless parallel wave.
        let out = TempDir::new().unwrap();
        let provider = two_module_provider();
        let providers: Vec<&dyn Provider> = vec![&provider];
        let reader = FakeVcsReader::new();
        let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let (_op, units) = sbom_operation(
            &request(),
            &document(),
            &providers,
            &readers,
            out.path(),
            &mut reporter,
        )
        .unwrap();

        assert_eq!(units.len(), 2);
        let edges: usize = units.iter().map(|unit| unit.depends_on.len()).sum();
        assert_eq!(
            edges, 1,
            "two modules of one shared target must be a serial chain: {units:?}"
        );
        // The chain is total: exactly one unit is a root, the other depends on it.
        let roots = units
            .iter()
            .filter(|unit| unit.depends_on.is_empty())
            .count();
        assert_eq!(roots, 1, "a serial chain has exactly one root: {units:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sbom_streams_a_settled_artifact_per_module() {
        // The engine streams a start + settled pair per module, each settled
        // event carrying that module's produced artifact — never buffering a
        // terminal table before first output.
        let out = TempDir::new().unwrap();
        let provider = two_module_provider();
        let providers: Vec<&dyn Provider> = vec![&provider];
        let reader = FakeVcsReader::new();
        let readers = MemberVcsReaders::single(&reader, BaselineSpec::explicit("main"));
        let mut reporter = RecordingReporter::new();

        let (op, units) = sbom_operation(
            &request(),
            &document(),
            &providers,
            &readers,
            out.path(),
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
        assert!(!summary.has_failures());
        assert_eq!(rec.started.len(), 2);
        assert_eq!(rec.settled.len(), 2);
        for (unit_id, status, outcome) in &rec.settled {
            assert_eq!(*status, toven_runtime::UnitStatus::Succeeded);
            let outcome = outcome.as_ref().expect("settled sbom carries an outcome");
            assert_eq!(&outcome.module.to_string(), unit_id);
            assert!(
                outcome.artifact.is_some(),
                "each module's SBOM artifact streams as its unit settles"
            );
        }
    }
}
