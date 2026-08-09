//! Standalone `release bump` phase coverage.
//!
//! Drives the public [`release_bump`] facade with per-member VCS doubles and
//! asserts the bump verb performs **only** the version + changelog mutation
//! half of a release: it rewrites manifests, rolls the configured changelog, and
//! **stages** the mutation for a pull request — never committing, tagging,
//! pushing, or publishing. `--dry-run` reports the planned mutation without
//! writing anything.

use std::collections::BTreeSet;

use rskit_util::time::FixedClock;
use rskit_version::semver::Version;
use toven_engine_core::config::{CanonicalRegistry, Document, load};
use toven_engine_core::federation::MemberVcsReaders;
use toven_engine_core::federation::baseline::MemberVcsReader;
use toven_engine_core::federation::member_repo::{MemberReleaseRepo, MemberReleaseRepos};
use toven_engine_core::plan::PlanRequest;
use toven_engine_release::{BumpOptions, BumpOverrides, release_bump};
use toven_model::{
    AbsPath, EcosystemId, Module, ModuleRef, RepoPath, ToolchainTag, Workspace, WorkspaceId,
};
use toven_ports::{
    ChangeRecord, ChangeStatus, DiscoverResponse, Oid, Provider, TagRef, TaskIntent,
};
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

/// Load a single-repo project whose `core` module is **maintainer-owned** — the
/// human already cut the tag/Release at the declared manifest version, so
/// `release bump` has nothing to advance on it.
fn load_maintainer_project() -> (toven_testkit::TestWorkspace, AbsPath, Document) {
    let ws = workspace("release-bump-maintainer");
    let body = "[project]\nname = \"solo\"\n\n[ecosystems.rust]\nmanifests = [\"Cargo.toml\"]\n\n[modules.\"rust:core\".release]\npush = false\nentrypoint = \"maintainer\"\ncommit_message = \"release {module} {version}\"\n";
    let path = ws
        .write_file("toven.toml", body.as_bytes())
        .expect("write project");
    let root = AbsPath::new(ws.path().to_path_buf()).expect("absolute root");
    let document = load(&path, &BTreeSet::new(), &CanonicalRegistry::model())
        .expect("project loads")
        .document;
    (ws, root, document)
}

/// A release tag anchoring `core` at `version` in the default `rust/core@`
/// scheme the fake target reports.
fn core_tag(version: &str) -> TagRef {
    TagRef::new(format!("rust/core@{version}"), Oid::new("cafe"))
}

/// Load a single-repo project whose `core` module declares a version reference
/// pinning itself in `README.md`, seeding the README with `readme` content.
fn load_version_reference_project(
    readme: &str,
) -> (toven_testkit::TestWorkspace, AbsPath, Document) {
    let ws = workspace("release-bump-version-ref");
    let body = "[project]\nname = \"solo\"\n\n[ecosystems.rust]\nmanifests = [\"Cargo.toml\"]\n\n[modules.\"rust:core\".release]\npush = false\ncommit_message = \"release {module} {version}\"\n\n[[modules.\"rust:core\".release.version_references]]\nfiles = [\"README.md\"]\npattern = \"{module} = \\\"{version}\\\"\"\n";
    let path = ws
        .write_file("toven.toml", body.as_bytes())
        .expect("write project");
    ws.write_file("README.md", readme.as_bytes())
        .expect("write readme");
    let root = AbsPath::new(ws.path().to_path_buf()).expect("absolute root");
    let document = load(&path, &BTreeSet::new(), &CanonicalRegistry::model())
        .expect("project loads")
        .document;
    (ws, root, document)
}

/// Build the single-`core` provider around a release target declaring `version`.
fn provider_declaring(version: Version) -> FakeProvider {
    provider_with_target(FakeReleaseTarget::new().with_declared_version(version))
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
) -> toven_engine_release::BumpReport {
    run_bump_with(ws, root, document, writer, options, &core_provider())
}

fn run_bump_with(
    ws: &toven_testkit::TestWorkspace,
    root: AbsPath,
    document: &Document,
    writer: &FakeVcsWriter,
    options: BumpOptions,
    provider: &FakeProvider,
) -> toven_engine_release::BumpReport {
    run_bump_reader(
        ws,
        root,
        document,
        writer,
        options,
        provider,
        &FakeVcsReader::new(),
    )
}

fn run_bump_reader(
    ws: &toven_testkit::TestWorkspace,
    root: AbsPath,
    document: &Document,
    writer: &FakeVcsWriter,
    options: BumpOptions,
    provider: &FakeProvider,
    reader: &FakeVcsReader,
) -> toven_engine_release::BumpReport {
    let providers: Vec<&dyn Provider> = vec![provider];
    let (readers, repos) = single_member(ws, reader, writer);
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

/// No bump run may ever commit, tag, push, or publish — bump only ever stages
/// the mutation for a pull request; the commit/tag/push half is `release tag` /
/// `release publish`.
fn assert_no_release_history(log: &[VcsWrite]) {
    assert!(
        !log.iter().any(|write| matches!(
            write,
            VcsWrite::Commit { .. } | VcsWrite::CreateTag { .. } | VcsWrite::Push { .. }
        )),
        "bump must never commit, tag, or push: {log:?}"
    );
}

#[test]
fn bump_stages_the_release_by_default() {
    // The default `release bump` stages the version/changelog mutation for a
    // pull request — it never creates the release commit. Cutting the commit is
    // the job of `release tag` / `release publish` after the staged change
    // merges (bump → branch → PR → merge → tag/publish).
    let (ws, root, document) = load_project(false);
    let writer = FakeVcsWriter::new().with_commit_oid("unused");
    let report = run_bump(&ws, root, &document, &writer, BumpOptions::default());

    assert!(report.staged, "the default bump stages the mutation");
    assert!(!report.dry_run);
    assert_eq!(report.modules.len(), 1, "one module is bumped: {report:?}");

    let log = writer.writes();
    assert_eq!(stages(&log), 1, "the mutation is staged: {log:?}");
    assert_eq!(
        commits(&log),
        0,
        "the default bump creates no commit: {log:?}"
    );
    assert_no_release_history(&log);
}

#[test]
fn dry_run_previews_without_writing() {
    let (ws, root, document) = load_project(true);
    let writer = FakeVcsWriter::new();
    let options = BumpOptions { dry_run: true };
    let report = run_bump(&ws, root, &document, &writer, options);

    assert!(report.dry_run);
    assert!(!report.staged, "a preview stages nothing");
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
            VcsWrite::Stage { paths, .. } if paths.iter().any(|path| path == "CHANGELOG.md")
        )),
        "the rolled changelog is staged for the pull request: {log:?}"
    );
    assert_no_release_history(&log);
}

#[test]
fn a_tag_only_bump_that_rewrites_nothing_stages_nothing() {
    // A tag-only ecosystem rewrites no manifest and (here) rolls no changelog,
    // so the default `bump` has nothing to stage. `staged` must reflect that no
    // mutation reached the working tree.
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
        !report.staged,
        "a bump that rewrote nothing must not claim a stage: {report:?}"
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

#[test]
fn an_all_maintainer_owned_workspace_with_no_change_is_a_no_op() {
    // rskit-shaped case: every module is maintainer-owned and its manifest
    // already declares the released version (a tag anchors it), with nothing
    // changed since. The publish path force-includes maintainer-owned modules to
    // verify their tags, but `bump` must not — a bump that advances nothing does
    // nothing: no plan, no rewrite, no changelog roll, no stage.
    let (ws, root, document) = load_maintainer_project();
    let writer = FakeVcsWriter::new().with_commit_oid("unused");
    let reader = FakeVcsReader::new().with_tags(vec![core_tag("0.1.0")]);
    let report = run_bump_reader(
        &ws,
        root,
        &document,
        &writer,
        BumpOptions::default(),
        &provider_declaring(Version::new(0, 1, 0)),
        &reader,
    );

    assert!(
        report.modules.is_empty(),
        "nothing advanced, so no module is bumped: {report:?}"
    );
    assert!(
        report.changelogs.is_empty(),
        "no changelog is rolled: {report:?}"
    );
    assert!(!report.staged, "a no-op bump stages nothing: {report:?}");
    assert!(!report.dry_run);

    let log = writer.writes();
    assert!(
        log.is_empty(),
        "a no-op bump leaves the working tree untouched — no stage, no commit: {log:?}"
    );
    assert_no_release_history(&log);
}

#[test]
fn a_module_changed_since_its_tag_still_enters_the_bump_plan() {
    // The change-gate keeps modules that genuinely advanced: a module with a real
    // Conventional-Commit change since its release tag is bumped even though the
    // maintainer-owned force-include is gone.
    let (ws, root, document) = load_project(false);
    let writer = FakeVcsWriter::new().with_commit_oid("bump-commit");
    let reader = FakeVcsReader::new()
        .with_tags(vec![core_tag("0.1.0")])
        .with_changed_since(vec![ChangeRecord::new(
            "src/lib.rs",
            ChangeStatus::Modified,
        )]);
    let report = run_bump_reader(
        &ws,
        root,
        &document,
        &writer,
        BumpOptions::default(),
        &provider_declaring(Version::new(0, 1, 0)),
        &reader,
    );

    assert_eq!(
        report.modules.len(),
        1,
        "the changed module still enters the bump plan: {report:?}"
    );
    assert_eq!(report.modules[0].old_version, Version::new(0, 1, 0));
    assert_eq!(
        report.modules[0].new_version,
        Version::new(0, 1, 1),
        "a real change advances the version: {report:?}"
    );
    assert!(report.staged, "the changed module is staged: {report:?}");
}

/// Extract the staged repo-relative paths recorded by the writer double.
fn staged_paths(log: &[VcsWrite]) -> Vec<String> {
    log.iter()
        .filter_map(|write| match write {
            VcsWrite::Stage { paths, .. } => Some(paths.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

#[test]
fn bump_syncs_a_declared_version_reference() {
    // A genuinely-changed module bumps 0.1.0 -> 0.1.1; its README pin is
    // rewritten to the authoritative post-bump version, inside the bump
    // mutation and staged with the manifest. A prose line mentioning a
    // version-shaped string but not matching the pin pattern is untouched.
    let (ws, root, document) = load_version_reference_project(
        "# core\n\ncore = \"0.1.0\"\n\nWe shipped core 0.0.9 last week.\n",
    );
    let writer = FakeVcsWriter::new().with_commit_oid("bump-commit");
    let reader = FakeVcsReader::new()
        .with_tags(vec![core_tag("0.1.0")])
        .with_changed_since(vec![ChangeRecord::new(
            "src/lib.rs",
            ChangeStatus::Modified,
        )]);
    let report = run_bump_reader(
        &ws,
        root,
        &document,
        &writer,
        BumpOptions::default(),
        &provider_declaring(Version::new(0, 1, 0)),
        &reader,
    );

    assert!(report.staged, "the mutation is staged: {report:?}");
    assert_eq!(report.modules[0].new_version, Version::new(0, 1, 1));

    let readme = std::fs::read_to_string(ws.path().join("README.md")).expect("readme");
    assert!(
        readme.contains("core = \"0.1.1\""),
        "the pin is synced to the post-bump version: {readme}"
    );
    assert!(
        readme.contains("We shipped core 0.0.9 last week."),
        "prose that does not match the pin pattern is untouched: {readme}"
    );

    let staged = staged_paths(&writer.writes());
    assert!(
        staged.iter().any(|path| path == "README.md"),
        "the synced version reference is staged with the manifest: {staged:?}"
    );
    assert_no_release_history(&writer.writes());
}

#[test]
fn an_already_synced_version_reference_is_not_restaged() {
    // Idempotency: a README already at the authoritative version is left
    // byte-for-byte unchanged and never joins the staged set, so a re-run stages
    // no version-reference file.
    let (ws, root, document) = load_version_reference_project("# core\n\ncore = \"0.1.1\"\n");
    let writer = FakeVcsWriter::new().with_commit_oid("bump-commit");
    let reader = FakeVcsReader::new()
        .with_tags(vec![core_tag("0.1.0")])
        .with_changed_since(vec![ChangeRecord::new(
            "src/lib.rs",
            ChangeStatus::Modified,
        )]);
    let report = run_bump_reader(
        &ws,
        root,
        &document,
        &writer,
        BumpOptions::default(),
        &provider_declaring(Version::new(0, 1, 0)),
        &reader,
    );

    assert_eq!(report.modules[0].new_version, Version::new(0, 1, 1));
    let readme = std::fs::read_to_string(ws.path().join("README.md")).expect("readme");
    assert_eq!(
        readme, "# core\n\ncore = \"0.1.1\"\n",
        "an already-current reference is untouched: {readme}"
    );
    let staged = staged_paths(&writer.writes());
    assert!(
        !staged.iter().any(|path| path == "README.md"),
        "an unchanged reference is not staged: {staged:?}"
    );
}

#[test]
fn a_version_reference_only_change_does_not_trigger_a_bump() {
    // A worktree/diff whose only changed path is a declared version-reference
    // file must not seed a bump — the file follows versions, it does not drive
    // them (the native tool-generated-change filter).
    let (ws, root, document) = load_version_reference_project("# core\n\ncore = \"0.1.0\"\n");
    let writer = FakeVcsWriter::new().with_commit_oid("unused");
    let reader = FakeVcsReader::new()
        .with_tags(vec![core_tag("0.1.0")])
        .with_changed_since(vec![ChangeRecord::new("README.md", ChangeStatus::Modified)]);
    let report = run_bump_reader(
        &ws,
        root,
        &document,
        &writer,
        BumpOptions::default(),
        &provider_declaring(Version::new(0, 1, 0)),
        &reader,
    );

    assert!(
        report.modules.is_empty(),
        "a version-reference-only diff advances nothing: {report:?}"
    );
    assert!(!report.staged, "nothing is staged: {report:?}");
    assert!(writer.writes().is_empty(), "the working tree is untouched");
}
