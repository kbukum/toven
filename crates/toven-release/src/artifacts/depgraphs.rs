//! Read-only `release depgraphs` projection.
//!
//! Renders the validated dependency graph to a Graphviz DOT artifact inside a
//! bounded output directory, reusing the model's DOT renderer
//! ([`toven_model::graph::render`]) so the projection never re-encodes graph
//! syntax. It mutates nothing outside the output directory and returns a typed
//! artifact manifest for the reporter.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::safe_join;
use rskit_fs::sync_io::dir::create_all;
use rskit_fs::sync_io::file::write_atomic;
use tokio_util::sync::CancellationToken;
use toven_ports::{Provider, Reporter};
use toven_runtime::{Completed, UnitOperation, UnitSpec};

use crate::ArtifactManifest;
use toven_core::config::Document;
use toven_core::federation::resolve::PathDriverLocator;
use toven_core::plan::{PlanRequest, prepare_front};

/// Temp-file prefix for atomic depgraph writes.
const DEPGRAPH_TEMP_PREFIX: &str = "toven-depgraph";

/// A read-only projection of the dependency-graph artifacts written to disk.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DepgraphReport {
    /// The DOT artifacts written into the bounded output directory.
    pub artifacts: Vec<ArtifactManifest>,
}

impl DepgraphReport {
    /// Construct a depgraph report.
    #[must_use]
    pub const fn new(artifacts: Vec<ArtifactManifest>) -> Self {
        Self { artifacts }
    }
}

/// Render the project dependency graph as a DOT artifact under `out_dir`.
///
/// The whole validated federation graph is rendered to one DOT file named after
/// the project. The output directory is created if missing; nothing outside it
/// is touched.
///
/// # Errors
/// Propagates configuration/discovery/graph failures and output-directory or
/// artifact-write I/O failures.
pub fn release_depgraphs(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    out_dir: &Path,
    reporter: &mut dyn Reporter,
) -> AppResult<DepgraphReport> {
    let inputs = DepgraphInputs::gather(request, document, providers, out_dir, reporter)?;
    let mut artifacts = inputs
        .graphs
        .iter()
        .map(|graph| depgraph_for(&inputs, graph))
        .collect::<AppResult<Vec<_>>>()?;
    artifacts.sort_by(|left, right| left.label.cmp(&right.label));
    Ok(DepgraphReport::new(artifacts))
}

/// One dependency graph artifact resolved during GATHER.
struct DepgraphArtifact {
    id: String,
    label: String,
    path: PathBuf,
    rendered: String,
}

/// Shared prerequisites for `release depgraphs`, resolved once before streaming.
pub struct DepgraphInputs {
    graphs: Vec<DepgraphArtifact>,
}

impl DepgraphInputs {
    /// Resolve the dependency graph DOT artifacts and create the bounded output directory.
    ///
    /// # Errors
    /// Propagates configuration/discovery/graph failures and output-directory failures.
    pub fn gather(
        request: &PlanRequest,
        document: &Document,
        providers: &[&dyn Provider],
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
        create_all(out_dir)?;
        let label = document.project.name.clone();
        let file_name = format!("{}.dot", sanitize_stem(&label));
        let path = safe_join(out_dir, &file_name).map_err(|error| {
            AppError::invalid_input(
                "release.out-dir",
                format!("depgraph artifact '{file_name}' escapes the output directory: {error}"),
            )
        })?;
        let rendered = toven_model::graph::render(&context.graph);
        Ok(Self {
            graphs: vec![DepgraphArtifact {
                id: label.clone(),
                label,
                path,
                rendered,
            }],
        })
    }

    fn graph(&self, id: &str) -> Option<&DepgraphArtifact> {
        self.graphs.iter().find(|graph| graph.id == id)
    }

    /// The engine unit graph: one independent unit per dependency graph artifact.
    #[must_use]
    pub fn units(&self) -> Vec<UnitSpec> {
        self.graphs
            .iter()
            .map(|graph| UnitSpec::new(graph.id.clone(), Vec::<String>::new()))
            .collect()
    }
}

/// One settled dependency-graph artifact.
pub type DepgraphOutcome = ArtifactManifest;

fn depgraph_for(_inputs: &DepgraphInputs, graph: &DepgraphArtifact) -> AppResult<DepgraphOutcome> {
    write_atomic(&graph.path, graph.rendered.as_bytes(), DEPGRAPH_TEMP_PREFIX)?;
    Ok(ArtifactManifest::new(
        graph.label.clone(),
        graph.path.clone(),
    ))
}

/// The `release depgraphs` per-unit operation on the shared runtime engine.
pub struct DepgraphOperation {
    inputs: Arc<DepgraphInputs>,
}

impl DepgraphOperation {
    /// Wrap gathered inputs as a runnable operation.
    #[must_use]
    pub fn new(inputs: DepgraphInputs) -> Self {
        Self {
            inputs: Arc::new(inputs),
        }
    }

    /// Share the gathered inputs.
    #[must_use]
    pub fn inputs(&self) -> Arc<DepgraphInputs> {
        Arc::clone(&self.inputs)
    }
}

#[async_trait]
impl UnitOperation for DepgraphOperation {
    type Shared = Arc<DepgraphInputs>;
    type Outcome = DepgraphOutcome;

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
            let graph = shared.graph(&id).ok_or_else(|| {
                AppError::new(ErrorCode::Internal, format!("unknown depgraph unit '{id}'"))
            })?;
            depgraph_for(&shared, graph)
        })
        .await
        .map_err(AppError::internal)??;
        Ok(Completed::succeeded(outcome))
    }
}

/// Build the `release depgraphs` operation and its engine unit graph.
///
/// # Errors
/// Propagates GATHER failures.
pub fn depgraph_operation(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    out_dir: &Path,
    reporter: &mut dyn Reporter,
) -> AppResult<(DepgraphOperation, Vec<UnitSpec>)> {
    let inputs = DepgraphInputs::gather(request, document, providers, out_dir, reporter)?;
    let units = inputs.units();
    Ok((DepgraphOperation::new(inputs), units))
}

/// Reduce a label to a filesystem-safe file stem: any non-alphanumeric run
/// collapses to a single `-`, so a member-scoped key never writes outside the
/// bounded directory or spawns nested directories.
fn sanitize_stem(label: &str) -> String {
    let mut stem = String::with_capacity(label.len());
    let mut last_dash = false;
    for ch in label.chars() {
        if ch.is_ascii_alphanumeric() {
            stem.push(ch);
            last_dash = false;
        } else if !last_dash {
            stem.push('-');
            last_dash = true;
        }
    }
    let trimmed = stem.trim_matches('-');
    if trimmed.is_empty() {
        "graph".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use rskit_config::RawValue;
    use rskit_fs::TempDir;
    use rskit_fs::sync_io::file::read_string_bounded;
    use serde_json::json;
    use toven_model::{AbsPath, EcosystemId, Module, ModuleRef, RepoPath};
    use toven_ports::{DiscoverResponse, Provider, TaskIntent};
    use toven_testkit::{FakeConfiguredAdapter, FakeProvider, RecordingReporter};

    use super::{DepgraphOutcome, depgraph_operation, release_depgraphs, sanitize_stem};
    use toven_core::config::{Document, ProjectConfig, TovenConfig};
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

    fn request() -> PlanRequest {
        PlanRequest::new(
            "r1",
            "demo",
            TaskIntent::resolve("release"),
            AbsPath::new("/repo").unwrap(),
        )
    }

    #[test]
    fn depgraphs_writes_a_dot_artifact_into_the_bounded_dir() {
        let core = module("core");
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![core];
        let adapter = FakeConfiguredAdapter::new(eid("rust")).with_response(response);
        let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
        let providers: Vec<&dyn Provider> = vec![&provider];

        let out = TempDir::new().unwrap();
        let mut reporter = RecordingReporter::new();

        let report = release_depgraphs(
            &request(),
            &document(),
            &providers,
            out.path(),
            &mut reporter,
        )
        .unwrap();

        assert_eq!(report.artifacts.len(), 1);
        let artifact = &report.artifacts[0];
        assert_eq!(artifact.label, "demo");
        assert!(artifact.path.starts_with(out.path()));
        let contents = read_string_bounded(&artifact.path, 64 * 1024).unwrap();
        assert!(contents.starts_with("digraph toven {"));
        assert!(contents.contains("\"rust:core\";"));
    }

    #[derive(Default)]
    struct Recorder {
        started: Vec<String>,
        settled: Vec<(String, toven_runtime::UnitStatus, Option<DepgraphOutcome>)>,
    }

    impl toven_runtime::Progress<DepgraphOutcome> for Recorder {
        fn started(&mut self, unit_id: &str) -> rskit_errors::AppResult<()> {
            self.started.push(unit_id.to_string());
            Ok(())
        }

        fn settled(
            &mut self,
            report: &toven_runtime::UnitReport<DepgraphOutcome>,
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
    fn depgraph_units_are_edgeless_one_per_artifact() {
        let core = module("core");
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![core];
        let adapter = FakeConfiguredAdapter::new(eid("rust")).with_response(response);
        let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let out = TempDir::new().unwrap();
        let mut reporter = RecordingReporter::new();

        let (_op, units) = depgraph_operation(
            &request(),
            &document(),
            &providers,
            out.path(),
            &mut reporter,
        )
        .unwrap();

        assert_eq!(units.len(), 1);
        assert!(units.iter().all(|unit| unit.depends_on.is_empty()));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn depgraph_streams_a_settled_artifact_per_unit() {
        let core = module("core");
        let mut response = DiscoverResponse::new(eid("rust"));
        response.modules = vec![core];
        let adapter = FakeConfiguredAdapter::new(eid("rust")).with_response(response);
        let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let out = TempDir::new().unwrap();
        let mut reporter = RecordingReporter::new();

        let (op, units) = depgraph_operation(
            &request(),
            &document(),
            &providers,
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

        assert_eq!(summary.total, 1);
        assert_eq!(summary.succeeded, 1);
        assert_eq!(rec.started.len(), 1);
        assert_eq!(rec.settled.len(), 1);
        let (_id, status, outcome) = &rec.settled[0];
        assert_eq!(*status, toven_runtime::UnitStatus::Succeeded);
        assert!(outcome.as_ref().unwrap().path.is_file());
    }

    #[test]
    fn sanitize_stem_collapses_unsafe_runs() {
        assert_eq!(sanitize_stem("lib/rust:core"), "lib-rust-core");
        assert_eq!(sanitize_stem("///"), "graph");
    }
}
