//! Read-only `release sbom` projection.
//!
//! Orchestrates each releasable module's ecosystem SBOM tool argv-first via its
//! [`SbomProducer`](toven_ports::SbomProducer), collecting the produced
//! `CycloneDX` artifacts into a bounded output directory. Toven owns scope,
//! ordering, and reporting; the ecosystem target owns the tool invocation. A
//! module whose ecosystem has no SBOM tooling is recorded as skipped rather
//! than failing the whole projection.

use std::path::Path;

use rskit_errors::{AppError, AppResult};
use rskit_fs::safe_join;
use rskit_fs::sync_io::dir::create_all;
use rskit_fs::sync_io::file::copy as copy_file;
use toven_model::ModuleKey;
use toven_ports::{Provider, Reporter};

use super::ArtifactManifest;
use super::plan::{release_targets, resolve_release_settings};
use crate::config::Document;
use crate::federation::resolve::PathDriverLocator;
use crate::plan::{PlanRequest, prepare_front};

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
/// A module is releasable when its ecosystem adapter exposes a release target.
/// Each target's SBOM tool is invoked argv-first, bounded to `out_dir`; a
/// target with no SBOM tooling contributes a skip. The output directory is
/// created if missing; nothing outside it is touched.
///
/// # Errors
/// Propagates configuration/discovery/graph failures, output-directory I/O
/// failures, and SBOM tool spawn/exit failures.
pub fn release_sbom(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    out_dir: &Path,
    reporter: &mut dyn Reporter,
) -> AppResult<SbomReport> {
    let locator = PathDriverLocator::new();
    let context = prepare_front(
        &request.project_root,
        document,
        providers,
        &locator,
        reporter,
    )?;
    let targets = release_targets(&context)?;

    create_all(out_dir)?;
    let mut artifacts = Vec::new();
    let mut skipped = Vec::new();
    for module in &context.federation.modules {
        let key = (module.member.clone(), module.id.ecosystem.clone());
        let Some(target) = targets.get(&key) else {
            continue;
        };
        match target.sbom(module, out_dir)? {
            Some(artifact) => {
                artifacts.push(ArtifactManifest::new(
                    module.key().to_string(),
                    artifact.path,
                ));
            }
            None => skipped.push(module.key()),
        }
    }
    artifacts.sort_by(|left, right| left.label.cmp(&right.label));
    skipped.sort();

    let settings = resolve_release_settings(&context, &targets)?;
    let staged = stage_sbom_assets(request.project_root.as_path(), &settings, &artifacts)?;

    Ok(SbomReport {
        artifacts,
        skipped,
        staged,
    })
}

/// Stage every declared `*.cdx.json` hosted-release asset from the produced SBOM
/// artifact whose file stem matches the asset. Fail-closed when a declared SBOM
/// asset has no matching produced artifact so the release surface never ships a
/// missing SBOM.
fn stage_sbom_assets(
    project_root: &Path,
    settings: &std::collections::BTreeMap<ModuleKey, super::ResolvedReleaseSettings>,
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
        CommonEcosystemConfig, DiscoverResponse, HostConfig, Provider, ReleaseConfig, TaskIntent,
    };
    use toven_testkit::{
        FakeConfiguredAdapter, FakeProvider, FakeReleaseTarget, RecordingReporter,
    };

    use super::release_sbom;
    use crate::config::{Document, ProjectConfig, TovenConfig};
    use crate::plan::PlanRequest;

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

        let out = TempDir::new().unwrap();
        let mut reporter = RecordingReporter::new();

        let report = release_sbom(
            &request(),
            &document(),
            &providers,
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

        let out = TempDir::new().unwrap();
        let mut reporter = RecordingReporter::new();

        let report = release_sbom(
            &request(),
            &document(),
            &providers,
            out.path(),
            &mut reporter,
        )
        .unwrap();

        assert!(report.artifacts.is_empty());
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0], module("core").key());
    }

    #[test]
    fn sbom_propagates_a_tool_failure() {
        let target = FakeReleaseTarget::new().with_sbom_failure("cyclonedx exploded");
        let provider = providers_with(target);
        let providers: Vec<&dyn Provider> = vec![&provider];

        let out = TempDir::new().unwrap();
        let mut reporter = RecordingReporter::new();

        let error = release_sbom(
            &request(),
            &document(),
            &providers,
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

        let out = TempDir::new().unwrap();
        let mut reporter = RecordingReporter::new();

        let report = release_sbom(
            &request_at(root.path()),
            &document(),
            &providers,
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

        let out = TempDir::new().unwrap();
        let mut reporter = RecordingReporter::new();

        let error = release_sbom(
            &request_at(root.path()),
            &document(),
            &providers,
            out.path(),
            &mut reporter,
        )
        .expect_err("a declared SBOM asset with no artifact must fail closed");
        assert!(error.to_string().contains("missing.cdx.json"));
    }
}
