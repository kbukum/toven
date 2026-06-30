//! `toven generate` flow tests: minimal first-run emit, additive/idempotent
//! re-run, `--force` regeneration, the bootstrap PATH-driver probe, the
//! generated-config → loader round-trip, and a generate → PLAN smoke.
//!
//! Discovery is faked through `toven-testkit` doubles; the PATH-probe transport
//! is exercised through an injected [`DriverScaffolder`]/[`DriverLocator`] pair
//! (no real subprocess), and the existing-config re-run reads
//! a shared fixture rather than inline TOML.

mod common;

use std::collections::BTreeSet;
use std::path::PathBuf;

use common::eid;
use rskit_fs::TempDir;
use rskit_fs::sync_io::file::{read_string, write};
use toml::{Table, Value};
use toven_engine::config::{CanonicalRegistry, load};
use toven_engine::federation::MemberVcsReaders;
use toven_engine::generate::generate_with;
use toven_engine::plan::{NullCache, PlanHost, PlanRequest, plan};
use toven_model::{
    AbsPath, DepKind, Edge, Module, ModuleRef, RepoPath, ToolchainTag, Workspace, WorkspaceId,
};
use toven_ports::{DiscoverResponse, EcosystemFragment, FanOut, Provider, Task, TaskKind};
use toven_testkit::{
    CountingToolchainProber, FakeConfiguredAdapter, FakeDriverLocator, FakeDriverScaffolder,
    FakeProvider, FakeSourceDigest, FakeVcsReader, RecordingReporter, fixtures,
};

/// Build a minimal `[ecosystems.<id>]` fragment carrying discovery `manifests`.
fn fragment(id: &str, manifests: &[&str]) -> EcosystemFragment {
    let mut table = Table::new();
    table.insert(
        "manifests".to_string(),
        Value::Array(
            manifests
                .iter()
                .map(|manifest| Value::String((*manifest).to_string()))
                .collect(),
        ),
    );
    EcosystemFragment::new(eid(id), table)
}

/// A provider that scaffolds `id` with the given discovery manifests.
fn scaffolding_provider(id: &str, manifests: &[&str]) -> FakeProvider {
    FakeProvider::new(eid(id)).with_scaffold(Some(fragment(id, manifests)))
}

#[test]
fn minimal_first_run_emits_project_and_ecosystem_only() {
    let dir = TempDir::new().expect("temp dir");
    let rust = scaffolding_provider("rust", &["Cargo.toml"]);
    let providers: Vec<&dyn Provider> = vec![&rust];

    let generated = generate_with(
        dir.path(),
        &providers,
        &FakeDriverScaffolder::new(),
        &FakeDriverLocator::new(),
        None,
        false,
    )
    .expect("generates");

    assert!(!generated.written, "no --write must not touch disk");
    assert!(!dir.path().join("toven.toml").exists());
    assert_eq!(generated.added, vec![eid("rust")]);
    assert!(generated.regenerated.is_empty());
    assert!(generated.warnings.is_empty());

    let rendered = &generated.rendered;
    assert!(rendered.contains("[project]"), "{rendered}");
    assert!(rendered.contains("[ecosystems.rust]"), "{rendered}");
    assert!(
        rendered.contains("manifests = [\"Cargo.toml\"]"),
        "{rendered}"
    );
    // Minimal: smart defaults stay implicit, so the run-strategy hint is commented.
    assert!(rendered.contains("# Uncomment to override"), "{rendered}");
    assert!(
        !rendered.contains("\nrun_strategy ="),
        "minimal emit must not dump default surface: {rendered}"
    );
}

#[test]
fn generated_config_round_trips_through_the_strict_loader() {
    let dir = TempDir::new().expect("temp dir");
    let rust = scaffolding_provider("rust", &["Cargo.toml"]);
    let providers: Vec<&dyn Provider> = vec![&rust];

    let generated = generate_with(
        dir.path(),
        &providers,
        &FakeDriverScaffolder::new(),
        &FakeDriverLocator::new(),
        None,
        true,
    )
    .expect("generates");
    assert!(generated.written);
    assert!(
        generated.created,
        "a first-run --write into an empty dir creates the file"
    );

    let loaded = load(
        &generated.path,
        &BTreeSet::from([eid("rust")]),
        &CanonicalRegistry::model(),
    )
    .expect("generated config parses through the live loader");

    assert!(!loaded.document.project.name.is_empty());
    assert_eq!(loaded.document.project.root(), ".");
    assert!(loaded.document.ecosystems.contains_key(&eid("rust")));
}

#[test]
fn first_run_write_with_no_detected_ecosystem_reports_a_create() {
    let dir = TempDir::new().expect("temp dir");
    // No in-proc providers and no PATH drivers: a project-only document.
    let providers: Vec<&dyn Provider> = Vec::new();

    let generated = generate_with(
        dir.path(),
        &providers,
        &FakeDriverScaffolder::new(),
        &FakeDriverLocator::new(),
        None,
        true,
    )
    .expect("generates");

    assert!(generated.written);
    assert!(
        generated.created,
        "writing a fresh project-only file is a create, not an up-to-date no-op"
    );
    assert!(generated.added.is_empty());
    assert!(generated.regenerated.is_empty());
    assert!(
        dir.path().join("toven.toml").is_file(),
        "the file was written to disk"
    );
}

#[test]
fn additive_rerun_adds_missing_warns_existing_and_preserves_project() {
    let dir = TempDir::new().expect("temp dir");
    let existing = fixtures::document_string("valid/generate-existing.toml").expect("fixture");
    let config = dir.path().join("toven.toml");
    write(&config, existing.as_bytes()).expect("seed existing config");

    let rust = scaffolding_provider("rust", &["Cargo.toml"]);
    let go = scaffolding_provider("go", &["go.mod"]);
    let providers: Vec<&dyn Provider> = vec![&rust, &go];

    let generated = generate_with(
        dir.path(),
        &providers,
        &FakeDriverScaffolder::new(),
        &FakeDriverLocator::new(),
        None,
        true,
    )
    .expect("generates");

    assert_eq!(
        generated.added,
        vec![eid("go")],
        "only the new section is added"
    );
    assert!(
        generated
            .warnings
            .iter()
            .any(|warning| warning.contains("[ecosystems.rust] already exists")),
        "{:?}",
        generated.warnings
    );

    let written = read_string(&config).expect("read merged config");
    // `[project]`/comments are never touched on an additive re-run.
    assert!(
        written.contains("This human comment must survive"),
        "{written}"
    );
    assert!(
        written.contains("name = \"already-onboarded\""),
        "{written}"
    );
    assert!(written.contains("base_ref = \"origin/trunk\""), "{written}");
    // The existing rust section is preserved verbatim; the new go section is added.
    assert!(
        written.contains("run_strategy = \"leaf-to-top\""),
        "{written}"
    );
    assert!(written.contains("[ecosystems.go]"), "{written}");
    assert!(written.contains("manifests = [\"go.mod\"]"), "{written}");
}

#[test]
fn rerun_without_changes_is_idempotent() {
    let dir = TempDir::new().expect("temp dir");
    let existing = fixtures::document_string("valid/generate-existing.toml").expect("fixture");
    let config = dir.path().join("toven.toml");
    write(&config, existing.as_bytes()).expect("seed existing config");

    let rust = scaffolding_provider("rust", &["Cargo.toml"]);
    let providers: Vec<&dyn Provider> = vec![&rust];

    let generated = generate_with(
        dir.path(),
        &providers,
        &FakeDriverScaffolder::new(),
        &FakeDriverLocator::new(),
        None,
        true,
    )
    .expect("generates");

    assert!(generated.added.is_empty());
    assert!(generated.regenerated.is_empty());
    assert!(
        !generated.created,
        "an additive re-run over an existing file does not report a create"
    );
    assert_eq!(
        read_string(&config).expect("read"),
        existing,
        "an additive re-run that adds nothing leaves the file byte-identical"
    );
}

#[test]
fn force_regenerates_exactly_one_section() {
    let dir = TempDir::new().expect("temp dir");
    let existing = fixtures::document_string("valid/generate-existing.toml").expect("fixture");
    let config = dir.path().join("toven.toml");
    write(&config, existing.as_bytes()).expect("seed existing config");

    let rust = scaffolding_provider("rust", &["crates/Cargo.toml"]);
    let providers: Vec<&dyn Provider> = vec![&rust];

    let generated = generate_with(
        dir.path(),
        &providers,
        &FakeDriverScaffolder::new(),
        &FakeDriverLocator::new(),
        Some("rust"),
        true,
    )
    .expect("generates");

    assert_eq!(generated.regenerated, vec![eid("rust")]);
    assert!(generated.added.is_empty());

    let written = read_string(&config).expect("read merged config");
    assert!(written.contains("crates/Cargo.toml"), "{written}");
    // A forced regenerate replaces the section, so its old *live* keys are gone
    // (a commented override hint mentioning run_strategy is expected, though).
    assert!(
        !written.contains("\nrun_strategy = \"leaf-to-top\""),
        "{written}"
    );
    // …but `[project]` and its comments are still untouched.
    assert!(
        written.contains("This human comment must survive"),
        "{written}"
    );
}

#[test]
fn bootstrap_probe_picks_up_a_path_driver() {
    let dir = TempDir::new().expect("temp dir");
    let rust = scaffolding_provider("rust", &["Cargo.toml"]);
    let providers: Vec<&dyn Provider> = vec![&rust];

    let go_driver = PathBuf::from("/fake/bin/toven-go");
    let locator = FakeDriverLocator::new().with_driver("toven-go", go_driver.clone());
    let scaffolder =
        FakeDriverScaffolder::new().with_fragments(go_driver, vec![fragment("go", &["go.mod"])]);

    let generated = generate_with(dir.path(), &providers, &scaffolder, &locator, None, false)
        .expect("generates");

    assert!(generated.added.contains(&eid("rust")));
    assert!(
        generated.added.contains(&eid("go")),
        "the PATH `toven-go` driver's fragment must be merged in: {:?}",
        generated.added
    );
    assert!(
        generated.rendered.contains("[ecosystems.go]"),
        "{}",
        generated.rendered
    );
}

#[test]
fn path_driver_scaffolding_a_foreign_ecosystem_is_rejected() {
    // A `toven-go` driver may only scaffold its own `go` ecosystem. A located
    // driver returning a fragment for a different ecosystem (here `rust`) is
    // misbehavior across the PATH-discovery trust boundary and must be a hard
    // error, never silently merged into the generated config.
    let dir = TempDir::new().expect("temp dir");
    let providers: Vec<&dyn Provider> = Vec::new();

    let go_driver = PathBuf::from("/fake/bin/toven-go");
    let locator = FakeDriverLocator::new().with_driver("toven-go", go_driver.clone());
    let scaffolder = FakeDriverScaffolder::new()
        .with_fragments(go_driver, vec![fragment("rust", &["Cargo.toml"])]);

    let error = generate_with(dir.path(), &providers, &scaffolder, &locator, None, false)
        .expect_err("a foreign-ecosystem fragment must be rejected");

    assert_eq!(error.code(), rskit_errors::ErrorCode::InvalidInput);
    let message = error.to_string();
    assert!(
        message.contains("toven-go") && message.contains("rust"),
        "error must name the misbehaving driver and the foreign ecosystem: {message}"
    );
}

#[test]
fn in_proc_provider_wins_over_a_path_driver_for_the_same_ecosystem() {
    let dir = TempDir::new().expect("temp dir");
    let go_in_proc = scaffolding_provider("go", &["go.mod"]);
    let providers: Vec<&dyn Provider> = vec![&go_in_proc];

    // A PATH `toven-go` exists too, but the in-proc provider is linked, so the
    // probe must never even consult the driver for `go`.
    let go_driver = PathBuf::from("/fake/bin/toven-go");
    let locator = FakeDriverLocator::new().with_driver("toven-go", go_driver.clone());
    let scaffolder = FakeDriverScaffolder::new()
        .with_fragments(go_driver, vec![fragment("go", &["DRIVER/go.mod"])]);

    let generated = generate_with(dir.path(), &providers, &scaffolder, &locator, None, false)
        .expect("generates");

    assert_eq!(generated.added, vec![eid("go")]);
    assert!(
        generated.rendered.contains("manifests = [\"go.mod\"]"),
        "{}",
        generated.rendered
    );
    assert!(
        !generated.rendered.contains("DRIVER/go.mod"),
        "the in-proc fragment must win: {}",
        generated.rendered
    );
}

#[test]
fn invalid_existing_config_is_a_typed_input_error() {
    // A re-run over a corrupt `toven.toml` must refuse with a typed InvalidInput
    // error, never silently overwrite the hand-maintained file.
    let dir = TempDir::new().expect("temp dir");
    let config = dir.path().join("toven.toml");
    write(&config, b"this is = not valid toml [[[").expect("seed broken config");

    let rust = scaffolding_provider("rust", &["Cargo.toml"]);
    let providers: Vec<&dyn Provider> = vec![&rust];

    let error = generate_with(
        dir.path(),
        &providers,
        &FakeDriverScaffolder::new(),
        &FakeDriverLocator::new(),
        None,
        true,
    )
    .expect_err("a corrupt existing config must not be silently overwritten");

    assert_eq!(error.code(), rskit_errors::ErrorCode::InvalidInput);
    // The broken file is left untouched (no atomic write happened).
    assert_eq!(
        read_string(&config).expect("read"),
        "this is = not valid toml [[[",
        "a refused merge must not touch the existing file"
    );
}

#[test]
fn force_unknown_ecosystem_warns_with_no_effect_on_first_run() {
    let dir = TempDir::new().expect("temp dir");
    let rust = scaffolding_provider("rust", &["Cargo.toml"]);
    let providers: Vec<&dyn Provider> = vec![&rust];

    let generated = generate_with(
        dir.path(),
        &providers,
        &FakeDriverScaffolder::new(),
        &FakeDriverLocator::new(),
        Some("python"),
        false,
    )
    .expect("generates");

    assert_eq!(generated.added, vec![eid("rust")]);
    assert!(generated.regenerated.is_empty());
    assert!(
        generated
            .warnings
            .iter()
            .any(|warning| warning.contains("--force 'python' had no effect")),
        "{:?}",
        generated.warnings
    );
}

#[test]
fn force_unknown_ecosystem_warns_with_no_effect_on_rerun() {
    let dir = TempDir::new().expect("temp dir");
    let existing = fixtures::document_string("valid/generate-existing.toml").expect("fixture");
    let config = dir.path().join("toven.toml");
    write(&config, existing.as_bytes()).expect("seed existing config");

    let rust = scaffolding_provider("rust", &["Cargo.toml"]);
    let providers: Vec<&dyn Provider> = vec![&rust];

    let generated = generate_with(
        dir.path(),
        &providers,
        &FakeDriverScaffolder::new(),
        &FakeDriverLocator::new(),
        Some("python"),
        true,
    )
    .expect("generates");

    assert!(
        generated
            .warnings
            .iter()
            .any(|warning| warning.contains("--force 'python' had no effect")),
        "{:?}",
        generated.warnings
    );
    // The forced id matched nothing, so the file is left byte-identical.
    assert_eq!(read_string(&config).expect("read"), existing);
}

/// A rust provider with a real two-module discovery graph for the PLAN smoke.
fn rust_plan_provider() -> FakeProvider {
    let mut response = DiscoverResponse::new(eid("rust"));
    response.workspaces.push(Workspace::new(
        WorkspaceId::new("rust").expect("workspace id"),
        RepoPath::new(".").expect("root"),
        ToolchainTag::new("cargo"),
    ));
    let mut errors = Module::new(
        ModuleRef::new(eid("rust"), "errors").expect("module ref"),
        RepoPath::new("crates/errors").expect("root"),
    );
    errors.workspace = Some(WorkspaceId::new("rust").expect("workspace id"));
    let mut app = Module::new(
        ModuleRef::new(eid("rust"), "app").expect("module ref"),
        RepoPath::new("crates/app").expect("root"),
    );
    app.workspace = Some(WorkspaceId::new("rust").expect("workspace id"));
    response.modules.push(errors);
    response.modules.push(app);
    response.edges.push(Edge::new(
        ModuleRef::new(eid("rust"), "app").expect("module ref"),
        ModuleRef::new(eid("rust"), "errors").expect("module ref"),
        DepKind::Normal,
    ));

    let adapter = FakeConfiguredAdapter::new(eid("rust"))
        .with_response(response)
        .with_tasks(vec![Task::new(
            TaskKind::Test,
            vec!["cargo".to_string(), "test".to_string()],
            FanOut::WholeWorkspace,
        )]);
    FakeProvider::new(eid("rust"))
        .with_adapter(adapter)
        .with_scaffold(Some(fragment("rust", &["Cargo.toml"])))
}

#[test]
fn generated_config_feeds_the_plan_spine() {
    let dir = TempDir::new().expect("temp dir");
    let rust = rust_plan_provider();
    let providers: Vec<&dyn Provider> = vec![&rust];

    let generated = generate_with(
        dir.path(),
        &providers,
        &FakeDriverScaffolder::new(),
        &FakeDriverLocator::new(),
        None,
        true,
    )
    .expect("generates");

    let document = load(
        &generated.path,
        &BTreeSet::from([eid("rust")]),
        &CanonicalRegistry::model(),
    )
    .expect("generated config loads")
    .document;

    let vcs = FakeVcsReader::new();
    let digest = FakeSourceDigest::new();
    let prober = CountingToolchainProber::new();
    let cache = NullCache;
    let mut reporter = RecordingReporter::new();
    let readers = MemberVcsReaders::single(&vcs, toven_ports::BaselineSpec::explicit("main"));
    let host = PlanHost::new(&readers, &digest, &prober, &cache);

    let request = PlanRequest::new(
        "run-1",
        "toven",
        TaskKind::Test,
        AbsPath::new("/repo").expect("absolute"),
    );
    let planned = plan(&request, &document, &providers, host, &mut reporter)
        .expect("the generated config plans cleanly");

    assert_eq!(
        planned.units.len(),
        1,
        "whole-workspace fan-out collapses both modules into one unit"
    );
    assert_eq!(
        planned.units[0].members.len(),
        2,
        "generate → plan must cover both modules"
    );
}
