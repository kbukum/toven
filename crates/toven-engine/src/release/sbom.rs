//! Read-only `release sbom` projection.
//!
//! Orchestrates each releasable module's ecosystem SBOM tool argv-first via its
//! [`ReleaseTarget`](toven_ports::ReleaseTarget), collecting the produced
//! `CycloneDX` artifacts into a bounded output directory. Toven owns scope,
//! ordering, and reporting; the ecosystem target owns the tool invocation. A
//! module whose ecosystem has no SBOM tooling is recorded as skipped rather
//! than failing the whole projection.

use std::path::Path;

use rskit_errors::AppResult;
use rskit_fs::sync_io::dir::create_all;
use toven_model::ModuleKey;
use toven_ports::{Provider, Reporter};

use super::ArtifactManifest;
use super::plan::release_targets;
use crate::config::Document;
use crate::federation::resolve::PathDriverLocator;
use crate::plan::{PlanRequest, prepare_front};

/// A read-only projection of the SBOM artifacts produced for the release scope.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SbomReport {
    /// The `CycloneDX` artifacts written into the bounded output directory.
    pub artifacts: Vec<ArtifactManifest>,
    /// Modules whose ecosystem has no SBOM tooling (skipped, not failed).
    pub skipped: Vec<ModuleKey>,
}

impl SbomReport {
    /// Construct an SBOM report.
    #[must_use]
    pub const fn new(artifacts: Vec<ArtifactManifest>, skipped: Vec<ModuleKey>) -> Self {
        Self { artifacts, skipped }
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
    Ok(SbomReport::new(artifacts, skipped))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use rskit_config::RawValue;
    use rskit_fs::TempDir;
    use serde_json::json;
    use toven_model::{AbsPath, EcosystemId, Module, ModuleRef, RepoPath};
    use toven_ports::{DiscoverResponse, Provider, TaskIntent};
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
}
