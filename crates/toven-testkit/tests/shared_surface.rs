//! Integration tests proving the shared `toven-testkit` surface end-to-end
//! through its **public** API.

use std::path::PathBuf;

use rskit_errors::ErrorCode;
use toven_model::AbsPath;
use toven_model::{EcosystemId, Event, UnitStatus};
use toven_ports::{
    BaselineSpec, ChangeRecord, ChangeStatus, DiscoverRequest, Provider, Reporter, VcsReader,
    VcsWriter,
};

use toven_testkit::doubles::VcsWrite;
use toven_testkit::{
    FakeProvider, FakeVcsReader, FakeVcsWriter, RecordingReporter, SampleRepo, assert_emitted,
    assert_event_sequence, document, ecosystem, repo_path,
};

#[test]
fn fixtures_api_loads_shared_tree() {
    let doc = document("valid/single-rust.toml").expect("loads valid document");
    assert_eq!(doc["project"]["name"].as_str(), Some("single-rust"));

    let adapter = ecosystem("rust", "adapter/cargo.toml").expect("loads ecosystem fixture");
    assert!(adapter.exists());

    let repo = repo_path("rust/single").expect("resolves seed repo");
    assert!(repo.join("toven.toml").exists());
}

#[test]
fn fixtures_api_rejects_missing_fixture_clearly() {
    let error = document("valid/nope.toml").unwrap_err();
    assert_eq!(error.code(), ErrorCode::NotFound);
    assert!(error.message().contains("not found"));
}

#[test]
fn sample_repo_materializes_and_git_inits() {
    let repo = SampleRepo::materialize("rust/single").expect("materializes seed repo");
    assert!(repo.root().join("toven.toml").exists());
    assert!(repo.child("crates/app/Cargo.toml").exists());

    let git = repo.init_git().expect("git inits with import commit");
    git.commit_file("crates/app/src/lib.rs", "pub fn f() {}", "add lib")
        .expect("second commit");
    git.tag("app@0.1.0", "release 0.1.0").expect("tags");

    assert!(git.has_tag("app@0.1.0").expect("tag present"));
    assert!(!git.resolve("HEAD").expect("resolves HEAD").is_empty());
}

#[test]
fn sample_repo_injects_shared_task_profiles() {
    // Fixture `toven.toml`s include `_profiles/<eco>-tasks.toml`; includes may
    // not traverse above the config root, so materialization must place the
    // shared profiles inside the materialized tree.
    let repo = SampleRepo::materialize("rust/single").expect("materializes seed repo");
    assert!(
        repo.child("_profiles/rust-tasks.toml").exists(),
        "shared profiles are injected into the materialized repo"
    );
}

#[test]
fn fake_vcs_reader_returns_scripted_changes() {
    let reader = FakeVcsReader::new()
        .with_changed_since(vec![ChangeRecord::new(
            "src/lib.rs",
            ChangeStatus::Modified,
        )])
        .with_ignored(vec![PathBuf::from("target")]);

    let changed = reader
        .changed_since(&BaselineSpec::merge_base("origin/main"))
        .expect("changed");
    assert_eq!(
        changed,
        vec![ChangeRecord::new("src/lib.rs", ChangeStatus::Modified)]
    );
    assert!(
        reader
            .is_ignored(std::path::Path::new("target"))
            .expect("ignored")
    );
}

#[test]
fn fake_vcs_writer_records_calls() {
    let writer = FakeVcsWriter::new().with_commit_oid("feed");
    writer.commit("release").expect("commit");
    writer
        .push("origin", &["refs/tags/v1".into()])
        .expect("push");

    assert_eq!(
        writer.writes(),
        vec![
            VcsWrite::Commit("release".into()),
            VcsWrite::Push {
                remote: "origin".into(),
                refspecs: vec!["refs/tags/v1".into()],
            },
        ]
    );
}

#[test]
fn fake_provider_drives_discovery() {
    let provider = FakeProvider::new(EcosystemId::new("rust").expect("id"));
    let configured = provider
        .configure(rskit_config::RawValue::Null)
        .expect("configures");

    let request = DiscoverRequest::new(AbsPath::new("/repo").expect("absolute"));
    let response = configured.discover(&request).expect("discovers");
    assert_eq!(response.schema_version, request.schema_version);
    assert_eq!(response.ecosystem.as_str(), "rust");
}

#[test]
fn recording_reporter_captures_event_order() {
    let mut reporter = RecordingReporter::new();
    for event in [
        Event::PlanPrepared { waves: 1, units: 1 },
        Event::UnitStarted {
            unit_id: "u1".into(),
        },
        Event::UnitFinished {
            unit_id: "u1".into(),
            status: UnitStatus::Succeeded,
        },
    ] {
        reporter.emit(&event).expect("emits");
    }

    assert_emitted(reporter.events(), |e| {
        matches!(e, Event::UnitStarted { .. })
    });
    assert_event_sequence(
        reporter.events(),
        &[
            Event::PlanPrepared { waves: 1, units: 1 },
            Event::UnitFinished {
                unit_id: "u1".into(),
                status: UnitStatus::Succeeded,
            },
        ],
    );
}
