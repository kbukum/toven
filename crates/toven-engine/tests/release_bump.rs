//! Standalone `release bump` phase coverage.
//!
//! Drives the public [`release_bump`] facade with per-member VCS doubles and
//! asserts the bump verb performs **only** the version + changelog mutation
//! half of a release: it rewrites manifests, rolls the configured changelog, and
//! either commits the release (the default) or stages it for a pull request
//! (`--no-commit`) — never tagging, pushing, or publishing. `--dry-run` reports
//! the planned mutation without writing anything.

use std::collections::BTreeSet;

use rskit_util::time::FixedClock;
use toven_engine::config::{CanonicalRegistry, Document, load};
use toven_engine::federation::MemberVcsReaders;
use toven_engine::federation::baseline::MemberVcsReader;
use toven_engine::federation::member_repo::{MemberReleaseRepo, MemberReleaseRepos};
use toven_engine::plan::PlanRequest;
use toven_engine::release::{BumpOptions, BumpOverrides, release_bump};
use toven_model::{
    AbsPath, EcosystemId, Module, ModuleRef, RepoPath, ToolchainTag, Workspace, WorkspaceId,
};
use toven_ports::{DiscoverResponse, Provider, TaskIntent};
use toven_testkit::workspace::workspace;
use toven_testkit::{
    FakeConfiguredAdapter, FakeProvider, FakeReleaseTarget, FakeVcsReader, FakeVcsWriter, VcsWrite,
};

/// `2024-06-15T00:00:00Z` — the fixed clock the bump verb stamps into a rolled
/// changelog heading.
const FIXED_EPOCH: u64 = 1_718_409_600;
const FIXED_DATE: &str = "2024-06-15";

fn eid(id: &str) -> EcosystemId {
    EcosystemId::new(id).expect("valid ecosystem id")
}

fn wsid(id: &str) -> WorkspaceId {
    WorkspaceId::new(id).expect("valid workspace id")
}

/// A rust provider exposing one releaseable `core` module rooted at the repo.
fn core_provider() -> FakeProvider {
    provider_with_target(FakeReleaseTarget::new())
}

/// A rust provider whose release target rewrites no manifest paths, modelling a
/// tag-only ecosystem (a Go-style version cut) that a `bump` leaves with nothing
/// to commit.
fn tag_only_provider() -> FakeProvider {
    provider_with_target(FakeReleaseTarget::new().with_written_paths(Vec::new()))
}

/// Build the single-`core`-module rust provider around a scripted release
/// target.
fn provider_with_target(target: FakeReleaseTarget) -> FakeProvider {
    let mut response = DiscoverResponse::new(eid("rust"));
    response.workspaces.push(Workspace::new(
        wsid("rust"),
        RepoPath::new(".").expect("root"),
        ToolchainTag::new("cargo"),
    ));
    let mut module = Module::new(
        ModuleRef::new(eid("rust"), "core").expect("ref"),
        RepoPath::new(".").expect("root"),
    );
    module.workspace = Some(wsid("rust"));
    response.modules.push(module);
    let adapter = FakeConfiguredAdapter::new(eid("rust"))
        .with_response(response)
        .with_release_target(target);
    FakeProvider::new(eid("rust")).with_adapter(adapter)
}

/// Load a single-repo project. When `roll` is set the module's release config
/// opts the changelog into rolling and a seed `CHANGELOG.md` with a documented
/// `[Unreleased]` section is written into the repo root.
fn load_project(roll: bool) -> (toven_testkit::TestWorkspace, AbsPath, Document) {
    let ws = workspace("release-bump");
    let changelog = if roll {
        "\n[modules.\"rust:core\".release.changelog]\npath = \"CHANGELOG.md\"\nroll = true\n"
    } else {
        ""
    };
    let body = format!(
        "[project]\nname = \"solo\"\n\n[ecosystems.rust]\nmanifests = [\"Cargo.toml\"]\n\n[modules.\"rust:core\".release]\npush = false\ncommit_message = \"release {{module}} {{version}}\"\n{changelog}",
    );
    let path = ws
        .write_file("toven.toml", body.as_bytes())
        .expect("write project");
    if roll {
        ws.write_file(
            "CHANGELOG.md",
            b"# Changelog\n\n## [Unreleased]\n\n### Added\n\n- A shiny new capability\n",
        )
        .expect("write changelog");
    }
    let root = AbsPath::new(ws.path().to_path_buf()).expect("absolute root");
    let document = load(&path, &BTreeSet::new(), &CanonicalRegistry::model())
        .expect("project loads")
        .document;
    (ws, root, document)
}

/// Build the single-member reader/repo bindings around one shared writer double.
fn single_member<'a>(
    ws: &toven_testkit::TestWorkspace,
    reader: &'a FakeVcsReader,
    writer: &'a FakeVcsWriter,
) -> (MemberVcsReaders<'a>, MemberReleaseRepos<'a>) {
    let readers = MemberVcsReaders::new(vec![MemberVcsReader::new(None, ".", None, reader)]);
    let repos = MemberReleaseRepos::new(vec![MemberReleaseRepo::new(
        None,
        ws.path().to_path_buf(),
        reader,
        writer,
    )]);
    (readers, repos)
}

fn request(root: AbsPath) -> PlanRequest {
    PlanRequest::new("bump-1", "solo", TaskIntent::resolve("release"), root)
}

fn run_bump(
    ws: &toven_testkit::TestWorkspace,
    root: AbsPath,
    document: &Document,
    writer: &FakeVcsWriter,
    options: BumpOptions,
) -> toven_engine::release::BumpReport {
    run_bump_with(ws, root, document, writer, options, &core_provider())
}

fn run_bump_with(
    ws: &toven_testkit::TestWorkspace,
    root: AbsPath,
    document: &Document,
    writer: &FakeVcsWriter,
    options: BumpOptions,
    provider: &FakeProvider,
) -> toven_engine::release::BumpReport {
    let providers: Vec<&dyn Provider> = vec![provider];
    let reader = FakeVcsReader::new();
    let (readers, repos) = single_member(ws, &reader, writer);
    let clock = FixedClock::new(FIXED_EPOCH, 0);
    let mut reporter = toven_testkit::RecordingReporter::new();
    release_bump(
        &request(root),
        document,
        &providers,
        &readers,
        &repos,
        &BumpOverrides::new(),
        &mut reporter,
        &clock,
        &options,
    )
    .expect("bump runs")
}

fn commits(log: &[VcsWrite]) -> usize {
    log.iter()
        .filter(|write| matches!(write, VcsWrite::Commit { .. }))
        .count()
}

fn stages(log: &[VcsWrite]) -> usize {
    log.iter()
        .filter(|write| matches!(write, VcsWrite::Stage { .. }))
        .count()
}

/// No bump run may ever tag, push, or publish — that is the tag/publish half.
fn assert_no_release_history(log: &[VcsWrite]) {
    assert!(
        !log.iter()
            .any(|write| matches!(write, VcsWrite::CreateTag { .. } | VcsWrite::Push { .. })),
        "bump must never tag or push: {log:?}"
    );
}

#[test]
fn bump_commits_the_release_by_default() {
    let (ws, root, document) = load_project(false);
    let writer = FakeVcsWriter::new().with_commit_oid("bump-commit");
    let report = run_bump(&ws, root, &document, &writer, BumpOptions::default());

    assert!(
        report.committed,
        "the default bump creates the release commit"
    );
    assert!(!report.dry_run);
    assert_eq!(report.modules.len(), 1, "one module is bumped: {report:?}");

    let log = writer.writes();
    assert_eq!(commits(&log), 1, "exactly one release commit: {log:?}");
    assert_eq!(stages(&log), 0, "the default path does not bare-stage");
    assert_no_release_history(&log);
    assert!(
        log.iter().any(|write| matches!(
            write,
            VcsWrite::Commit { message, .. } if message == "release core 0.1.0"
        )),
        "the configured commit message is used: {log:?}"
    );
}

#[test]
fn no_commit_stages_the_mutation_for_a_pull_request() {
    let (ws, root, document) = load_project(false);
    let writer = FakeVcsWriter::new();
    let options = BumpOptions {
        no_commit: true,
        dry_run: false,
    };
    let report = run_bump(&ws, root, &document, &writer, options);

    assert!(!report.committed, "--no-commit leaves the commit to the PR");
    let log = writer.writes();
    assert_eq!(stages(&log), 1, "the mutation is staged: {log:?}");
    assert_eq!(commits(&log), 0, "--no-commit creates no commit: {log:?}");
    assert_no_release_history(&log);
}

#[test]
fn dry_run_previews_without_writing() {
    let (ws, root, document) = load_project(true);
    let writer = FakeVcsWriter::new();
    let options = BumpOptions {
        no_commit: false,
        dry_run: true,
    };
    let report = run_bump(&ws, root, &document, &writer, options);

    assert!(report.dry_run);
    assert!(!report.committed, "a preview commits nothing");
    assert_eq!(report.modules.len(), 1, "the planned bump is reported");
    assert!(
        report.modules[0].manifests.is_empty(),
        "a preview reports no rewritten manifests: {report:?}"
    );
    assert_eq!(
        report.changelogs,
        vec!["CHANGELOG.md".to_string()],
        "the changelog that would roll is previewed"
    );

    let log = writer.writes();
    assert!(log.is_empty(), "a preview writes no git state: {log:?}");
    // The on-disk changelog is untouched by a preview.
    let text = std::fs::read_to_string(ws.path().join("CHANGELOG.md")).expect("changelog");
    assert!(
        text.contains("## [Unreleased]") && !text.contains(FIXED_DATE),
        "a preview must not roll the changelog on disk: {text}"
    );
}

#[test]
fn bump_rolls_the_configured_changelog() {
    let (ws, root, document) = load_project(true);
    let writer = FakeVcsWriter::new().with_commit_oid("bump-commit");
    let report = run_bump(&ws, root, &document, &writer, BumpOptions::default());

    assert_eq!(
        report.changelogs,
        vec!["CHANGELOG.md".to_string()],
        "the rolled changelog is reported: {report:?}"
    );
    let text = std::fs::read_to_string(ws.path().join("CHANGELOG.md")).expect("changelog");
    assert!(
        text.contains(&format!("## [0.1.0] - {FIXED_DATE}")),
        "the [Unreleased] body is rolled under a dated version heading: {text}"
    );
    assert!(
        text.contains("- A shiny new capability"),
        "the documented body is preserved verbatim: {text}"
    );

    let log = writer.writes();
    assert!(
        log.iter().any(|write| matches!(
            write,
            VcsWrite::Commit { paths, .. } if paths.iter().any(|path| path == "CHANGELOG.md")
        )),
        "the rolled changelog is staged into the release commit: {log:?}"
    );
    assert_no_release_history(&log);
}

#[test]
fn a_tag_only_bump_that_rewrites_nothing_reports_no_commit() {
    // A tag-only ecosystem rewrites no manifest and (here) rolls no changelog,
    // so the default `bump` has nothing to commit. `committed` must reflect that
    // no release commit was created, not the requested commit disposition.
    let (ws, root, document) = load_project(false);
    let writer = FakeVcsWriter::new().with_commit_oid("unused");
    let report = run_bump_with(
        &ws,
        root,
        &document,
        &writer,
        BumpOptions::default(),
        &tag_only_provider(),
    );

    assert!(
        !report.committed,
        "a bump that rewrote nothing must not claim a commit: {report:?}"
    );
    assert_eq!(report.modules.len(), 1, "the module is still planned");
    assert!(
        report.modules[0].manifests.is_empty(),
        "a tag-only ecosystem rewrote no manifest: {report:?}"
    );

    let log = writer.writes();
    assert_eq!(commits(&log), 0, "nothing is committed: {log:?}");
    assert_eq!(stages(&log), 0, "nothing is staged: {log:?}");
    assert_no_release_history(&log);
}
