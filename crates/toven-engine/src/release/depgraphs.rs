//! Read-only `release depgraphs` projection.
//!
//! Renders the validated dependency graph to a Graphviz DOT artifact inside a
//! bounded output directory, reusing the model's DOT renderer
//! ([`toven_model::graph::render`]) so the projection never re-encodes graph
//! syntax. It mutates nothing outside the output directory and returns a typed
//! artifact manifest for the reporter.

use std::path::Path;

use rskit_errors::{AppError, AppResult};
use rskit_fs::safe_join;
use rskit_fs::sync_io::dir::create_all;
use rskit_fs::sync_io::file::write_atomic;
use toven_ports::{Provider, Reporter};

use super::ArtifactManifest;
use crate::config::Document;
use crate::federation::resolve::PathDriverLocator;
use crate::plan::{PlanRequest, prepare_front};

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
    write_atomic(&path, rendered.as_bytes(), DEPGRAPH_TEMP_PREFIX)?;

    Ok(DepgraphReport::new(vec![ArtifactManifest::new(
        label, path,
    )]))
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

    use super::{release_depgraphs, sanitize_stem};
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

    #[test]
    fn sanitize_stem_collapses_unsafe_runs() {
        assert_eq!(sanitize_stem("lib/rust:core"), "lib-rust-core");
        assert_eq!(sanitize_stem("///"), "graph");
    }
}
