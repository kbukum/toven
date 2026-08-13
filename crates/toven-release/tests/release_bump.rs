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
use toven_core::config::{CanonicalRegistry, Document, load};
use toven_core::federation::MemberVcsReaders;
use toven_core::federation::baseline::MemberVcsReader;
use toven_core::federation::member_repo::{MemberReleaseRepo, MemberReleaseRepos};
use toven_core::plan::PlanRequest;
use toven_model::{
    AbsPath, EcosystemId, Module, ModuleRef, RepoPath, ToolchainTag, Workspace, WorkspaceId,
};
use toven_ports::{
    ChangeRecord, ChangeStatus, DiscoverResponse, Oid, Provider, TagRef, TaskIntent,
};
use toven_release::{BumpOptions, BumpOverrides, release_bump};
use toven_testkit::workspace::workspace;
use toven_testkit::{
    FakeConfiguredAdapter, FakeProvider, FakeReleaseTarget, FakeVcsReader, FakeVcsWriter,
    RecordingHookRunner, VcsWrite,
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

/// Load a single-repo project whose `core` module declares an `on-resolved`
/// bump hook running the `sync-extra` task reference.
fn load_on_resolved_project() -> (toven_testkit::TestWorkspace, AbsPath, Document) {
    let ws = workspace("release-bump-on-resolved");
    let body = "[project]\nname = \"solo\"\n\n[ecosystems.rust]\nmanifests = [\"Cargo.toml\"]\n\n[modules.\"rust:core\".release]\npush = false\ncommit_message = \"release {module} {version}\"\non_resolved = [\"sync-extra\"]\n";
    let path = ws
        .write_file("toven.toml", body.as_bytes())
        .expect("write project");
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
) -> toven_release::BumpReport {
    run_bump_with(ws, root, document, writer, options, &core_provider())
}

fn run_bump_with(
    ws: &toven_testkit::TestWorkspace,
    root: AbsPath,
    document: &Document,
    writer: &FakeVcsWriter,
    options: BumpOptions,
    provider: &FakeProvider,
) -> toven_release::BumpReport {
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
) -> toven_release::BumpReport {
    let providers: Vec<&dyn Provider> = vec![provider];
    let (readers, repos) = single_member(ws, reader, writer);
    let clock = FixedClock::new(FIXED_EPOCH, 0);
    let mut reporter = toven_testkit::RecordingReporter::new();
    let resolved = RecordingHookRunner::new();
    release_bump(
        &request(root),
        document,
        &providers,
        &readers,
        &repos,
        &BumpOverrides::new(),
        &mut reporter,
        &clock,
        &resolved,
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
fn a_pin_referencing_the_ecosystem_identity_is_synced() {
    // A pin may reference a module by its `ecosystem:name` identity (e.g.
    // `rust:core`), not only its bare package name; the authoritative map keys
    // both forms, so an identity-form pin is rewritten to the post-bump version.
    let (ws, root, document) = load_version_reference_project("# core\n\nrust:core = \"0.1.0\"\n");
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
    assert!(
        readme.contains("rust:core = \"0.1.1\""),
        "an ecosystem-identity pin is synced to the post-bump version: {readme}"
    );
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

#[test]
fn a_failed_version_reference_sync_restores_the_partially_mutated_member() {
    // Phase-1 undoable guarantee: if version-reference syncing fails after the
    // manifests/changelog were already written, the member must not be left
    // partially mutated. An oversized (unreadable) reference file aborts the
    // sync; the bump surfaces the error and restores the working tree.
    let oversized = format!(
        "# core\n\ncore = \"0.1.0\"\n{}",
        "a".repeat(5 * 1024 * 1024)
    );
    let (ws, root, document) = load_version_reference_project(&oversized);
    let writer = FakeVcsWriter::new().with_commit_oid("bump-commit");
    let reader = FakeVcsReader::new()
        .with_tags(vec![core_tag("0.1.0")])
        .with_changed_since(vec![ChangeRecord::new(
            "src/lib.rs",
            ChangeStatus::Modified,
        )]);

    let provider = provider_declaring(Version::new(0, 1, 0));
    let providers: Vec<&dyn Provider> = vec![&provider];
    let (readers, repos) = single_member(&ws, &reader, &writer);
    let clock = FixedClock::new(FIXED_EPOCH, 0);
    let mut reporter = toven_testkit::RecordingReporter::new();
    let resolved = RecordingHookRunner::new();
    let result = release_bump(
        &request(root),
        &document,
        &providers,
        &readers,
        &repos,
        &BumpOverrides::new(),
        &mut reporter,
        &clock,
        &resolved,
        &BumpOptions::default(),
    );

    assert!(
        result.is_err(),
        "an unreadable reference file aborts the bump: {result:?}"
    );
    assert!(
        writer
            .writes()
            .iter()
            .any(|write| matches!(write, VcsWrite::RestoreWorktree)),
        "the partially mutated member is restored: {:?}",
        writer.writes()
    );
    assert_no_release_history(&writer.writes());
}

#[test]
fn a_snapshot_read_failure_before_the_hooks_restores_the_member() {
    // The pre-hook untracked snapshot is a working-tree read that can itself
    // fail after phase-1 already mutated the member's tracked files. Because no
    // hook has run yet, there is nothing untracked to remove — but the tracked
    // mutation must still roll back so the failed bump leaves no partial state.
    let (ws, root, document) = load_on_resolved_project();
    let writer = FakeVcsWriter::new().with_commit_oid("bump-commit");
    // Fail the snapshot read: it is the first working-tree read after phase-1
    // prepares the member and before any hook runs (an empty tree at that
    // point, so a state-based fault cannot target it — the ordinal can).
    let reader = FakeVcsReader::new()
        .with_tags(vec![core_tag("0.1.0")])
        .with_changed_since(vec![ChangeRecord::new(
            "src/lib.rs",
            ChangeStatus::Modified,
        )])
        .with_worktree_status_failure_on_call(3, "snapshot read faulted");
    let resolved = RecordingHookRunner::producing(
        reader.worktree_handle(),
        vec![ChangeRecord::new("GENERATED.txt", ChangeStatus::Added)],
    );
    let result = run_bump_resolved(
        &ws,
        root,
        &document,
        &writer,
        &reader,
        &provider_declaring(Version::new(0, 1, 0)),
        &resolved,
    );

    assert!(
        result.is_err(),
        "a snapshot read failure aborts the bump: {result:?}"
    );
    assert!(
        resolved.resolved_calls().is_empty(),
        "the snapshot faults before any hook runs: {:?}",
        resolved.resolved_calls()
    );
    assert!(
        writer
            .writes()
            .iter()
            .any(|write| matches!(write, VcsWrite::RestoreWorktree)),
        "the mutated member is restored even when the snapshot read fails: {:?}",
        writer.writes()
    );
}

#[test]
fn a_join_read_failure_after_the_hooks_aborts_and_cleans_up() {
    // After the hooks succeed, joining their edits reads the working tree again;
    // that read can fault while the tree carries both the tracked mutation and
    // the hooks' brand-new untracked output. The abort must still delete the
    // untracked files and restore the tracked mutation — no partial state.
    let (ws, root, document) = load_on_resolved_project();
    let generated = ws
        .write_file("GENERATED.txt", b"scratch output\n")
        .expect("write generated file");
    assert!(generated.exists(), "the generated file exists pre-abort");

    let writer = FakeVcsWriter::new().with_commit_oid("bump-commit");
    // The hook succeeds and reports its untracked output, dirtying the tree;
    // the very next (join) read is the first non-empty read, and it faults.
    let reader = FakeVcsReader::new()
        .with_tags(vec![core_tag("0.1.0")])
        .with_changed_since(vec![ChangeRecord::new(
            "src/lib.rs",
            ChangeStatus::Modified,
        )])
        .with_worktree_status_failure_when_dirty("join read faulted");
    let resolved = RecordingHookRunner::producing(
        reader.worktree_handle(),
        vec![ChangeRecord::new("GENERATED.txt", ChangeStatus::Added)],
    );
    let result = run_bump_resolved(
        &ws,
        root,
        &document,
        &writer,
        &reader,
        &provider_declaring(Version::new(0, 1, 0)),
        &resolved,
    );

    assert!(
        result.is_err(),
        "a join read failure aborts the bump: {result:?}"
    );
    assert_eq!(
        resolved.resolved_calls().len(),
        1,
        "the join read faults only after the hook succeeded: {:?}",
        resolved.resolved_calls()
    );
    assert!(
        !generated.exists(),
        "the untracked file the hook created is removed on abort"
    );
    assert!(
        writer
            .writes()
            .iter()
            .any(|write| matches!(write, VcsWrite::RestoreWorktree)),
        "the mutated member is still restored: {:?}",
        writer.writes()
    );
}

/// Drive `release_bump` for a single member with an explicit `on-resolved`
/// runner and reader, returning the report (or the typed failure).
fn run_bump_resolved(
    ws: &toven_testkit::TestWorkspace,
    root: AbsPath,
    document: &Document,
    writer: &FakeVcsWriter,
    reader: &FakeVcsReader,
    provider: &FakeProvider,
    resolved: &RecordingHookRunner,
) -> rskit_errors::AppResult<toven_release::BumpReport> {
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
        resolved,
        &BumpOptions::default(),
    )
}

#[test]
fn an_on_resolved_hook_receives_the_version_map_and_stages_its_edits() {
    // The bump `on-resolved` seam runs after the version decision (core
    // 0.1.0 -> 0.1.1), is handed the authoritative version map, and its file
    // edit joins the staged set alongside the manifest.
    let (ws, root, document) = load_on_resolved_project();
    let writer = FakeVcsWriter::new().with_commit_oid("bump-commit");
    let reader = FakeVcsReader::new()
        .with_tags(vec![core_tag("0.1.0")])
        .with_changed_since(vec![ChangeRecord::new(
            "src/lib.rs",
            ChangeStatus::Modified,
        )]);
    // The scripted task produces a `VERSIONS.txt` edit into the shared worktree
    // handle on success — the reader reports it only after the clean-tree guard
    // has already observed an empty tree.
    let resolved = RecordingHookRunner::producing(
        reader.worktree_handle(),
        vec![ChangeRecord::new("VERSIONS.txt", ChangeStatus::Added)],
    );
    let report = run_bump_resolved(
        &ws,
        root,
        &document,
        &writer,
        &reader,
        &provider_declaring(Version::new(0, 1, 0)),
        &resolved,
    )
    .expect("bump runs");

    assert!(report.staged, "the mutation is staged: {report:?}");

    let calls = resolved.resolved_calls();
    assert_eq!(calls.len(), 1, "the on-resolved hook ran once: {calls:?}");
    assert_eq!(calls[0].reference, "sync-extra");
    assert!(
        calls[0].version_map_contents.contains("0.1.1")
            && calls[0].version_map_contents.contains("core"),
        "the hook received the authoritative post-bump version map: {}",
        calls[0].version_map_contents
    );

    let staged = staged_paths(&writer.writes());
    assert!(
        staged.iter().any(|path| path == "VERSIONS.txt"),
        "the on-resolved task's edit is staged with the manifest: {staged:?}"
    );
    assert_no_release_history(&writer.writes());
}

#[test]
fn a_preview_bump_never_runs_the_mutating_on_resolved_hook() {
    // Acceptance (mutation-free preview): a `--dry-run` bump previews the
    // mutation without writing, so the `on-resolved` seam — the mutating hook
    // form that edits the working tree and joins the staged set — must never
    // run. Only a gated (non-preview) apply may run it.
    let (ws, root, document) = load_on_resolved_project();
    let writer = FakeVcsWriter::new().with_commit_oid("bump-commit");
    let reader = FakeVcsReader::new()
        .with_tags(vec![core_tag("0.1.0")])
        .with_changed_since(vec![ChangeRecord::new(
            "src/lib.rs",
            ChangeStatus::Modified,
        )]);
    let resolved = RecordingHookRunner::producing(
        reader.worktree_handle(),
        vec![ChangeRecord::new("VERSIONS.txt", ChangeStatus::Added)],
    );

    let provider = provider_declaring(Version::new(0, 1, 0));
    let providers: Vec<&dyn Provider> = vec![&provider];
    let (readers, repos) = single_member(&ws, &reader, &writer);
    let clock = FixedClock::new(FIXED_EPOCH, 0);
    let mut reporter = toven_testkit::RecordingReporter::new();
    let report = release_bump(
        &request(root),
        &document,
        &providers,
        &readers,
        &repos,
        &BumpOverrides::new(),
        &mut reporter,
        &clock,
        &resolved,
        &BumpOptions { dry_run: true },
    )
    .expect("preview bump runs");

    assert!(report.dry_run, "the run is a preview: {report:?}");
    assert!(!report.staged, "a preview stages nothing: {report:?}");
    assert!(
        resolved.resolved_calls().is_empty(),
        "the mutating on-resolved hook never runs in preview: {:?}",
        resolved.resolved_calls()
    );
    assert!(
        writer.writes().is_empty(),
        "a preview writes nothing: {:?}",
        writer.writes()
    );
}

#[test]
fn a_failing_on_resolved_hook_aborts_and_restores_the_member() {
    // A failing `on-resolved` task fails the bump closed: the already-mutated
    // member is restored and nothing is staged.
    let (ws, root, document) = load_on_resolved_project();
    let writer = FakeVcsWriter::new().with_commit_oid("bump-commit");
    let reader = FakeVcsReader::new()
        .with_tags(vec![core_tag("0.1.0")])
        .with_changed_since(vec![ChangeRecord::new(
            "src/lib.rs",
            ChangeStatus::Modified,
        )]);
    let resolved = RecordingHookRunner::failing_on("sync-extra");
    let result = run_bump_resolved(
        &ws,
        root,
        &document,
        &writer,
        &reader,
        &provider_declaring(Version::new(0, 1, 0)),
        &resolved,
    );

    assert!(
        result.is_err(),
        "a failing on-resolved hook aborts the bump: {result:?}"
    );
    assert!(
        writer
            .writes()
            .iter()
            .any(|write| matches!(write, VcsWrite::RestoreWorktree)),
        "the mutated member is restored: {:?}",
        writer.writes()
    );
    assert!(
        !writer
            .writes()
            .iter()
            .any(|write| matches!(write, VcsWrite::Stage { .. })),
        "nothing is staged when the on-resolved hook fails: {:?}",
        writer.writes()
    );
}

#[test]
fn a_failing_on_resolved_hook_removes_the_untracked_files_it_created() {
    // A task that creates working-tree files and *then* fails must leave no
    // partial state. `restore_worktree` intentionally leaves untracked files in
    // place, so the abort path deletes the brand-new untracked files the hook
    // introduced before rolling the member's tracked mutation back.
    let (ws, root, document) = load_on_resolved_project();
    // The task's brand-new output exists on disk when the hook fails.
    let generated = ws
        .write_file("GENERATED.txt", b"scratch output\n")
        .expect("write generated file");
    assert!(generated.exists(), "the generated file exists pre-abort");

    let writer = FakeVcsWriter::new().with_commit_oid("bump-commit");
    let reader = FakeVcsReader::new()
        .with_tags(vec![core_tag("0.1.0")])
        .with_changed_since(vec![ChangeRecord::new(
            "src/lib.rs",
            ChangeStatus::Modified,
        )]);
    // The task reports the untracked file only after the clean-tree guard has
    // observed an empty tree, then fails.
    let resolved = RecordingHookRunner::producing_then_failing(
        reader.worktree_handle(),
        vec![ChangeRecord::new("GENERATED.txt", ChangeStatus::Added)],
        "sync-extra",
    );
    let result = run_bump_resolved(
        &ws,
        root,
        &document,
        &writer,
        &reader,
        &provider_declaring(Version::new(0, 1, 0)),
        &resolved,
    );

    assert!(
        result.is_err(),
        "a failing on-resolved hook aborts the bump: {result:?}"
    );
    assert!(
        !generated.exists(),
        "the untracked file the failing hook created is removed on abort"
    );
    assert!(
        writer
            .writes()
            .iter()
            .any(|write| matches!(write, VcsWrite::RestoreWorktree)),
        "the mutated member is still restored: {:?}",
        writer.writes()
    );
}

#[test]
fn an_on_resolved_abort_still_restores_when_untracked_cleanup_faults() {
    // The failing hook creates an untracked file, and the abort's cleanup status
    // read then faults. Cleanup is best-effort, so the tracked restore must still
    // run — the bump may not strand the manifest/changelog mutation just because
    // it could not delete the stray untracked file.
    let (ws, root, document) = load_on_resolved_project();
    let writer = FakeVcsWriter::new().with_commit_oid("bump-commit");
    let reader = FakeVcsReader::new()
        .with_tags(vec![core_tag("0.1.0")])
        .with_changed_since(vec![ChangeRecord::new(
            "src/lib.rs",
            ChangeStatus::Modified,
        )])
        // The pre-bump tree and pre-hook snapshot read clean; the first dirty
        // read — the abort's untracked-cleanup enumeration — faults.
        .with_worktree_status_failure_when_dirty("status read unavailable mid-abort");
    let resolved = RecordingHookRunner::producing_then_failing(
        reader.worktree_handle(),
        vec![ChangeRecord::new("GENERATED.txt", ChangeStatus::Added)],
        "sync-extra",
    );
    let result = run_bump_resolved(
        &ws,
        root,
        &document,
        &writer,
        &reader,
        &provider_declaring(Version::new(0, 1, 0)),
        &resolved,
    );

    assert!(
        result.is_err(),
        "a failing on-resolved hook aborts the bump: {result:?}"
    );
    assert!(
        writer
            .writes()
            .iter()
            .any(|write| matches!(write, VcsWrite::RestoreWorktree)),
        "the tracked mutation is restored even when untracked cleanup faults: {:?}",
        writer.writes()
    );
    assert!(
        !writer
            .writes()
            .iter()
            .any(|write| matches!(write, VcsWrite::Stage { .. })),
        "nothing is staged when the on-resolved hook fails: {:?}",
        writer.writes()
    );
}

#[test]
fn a_runaway_on_resolved_hook_that_floods_the_worktree_fails_closed() {
    // A hook that produces more than the untracked-path cap of working-tree files
    // must fail the join closed rather than build and stage an unbounded set.
    let (ws, root, document) = load_on_resolved_project();
    let writer = FakeVcsWriter::new().with_commit_oid("bump-commit");
    let reader = FakeVcsReader::new()
        .with_tags(vec![core_tag("0.1.0")])
        .with_changed_since(vec![ChangeRecord::new(
            "src/lib.rs",
            ChangeStatus::Modified,
        )]);
    // Just over the cap, reported only after the clean-tree guard observed an
    // empty tree, so the successful-hook join path hits the bound.
    let flood: Vec<ChangeRecord> = (0..=100_000)
        .map(|index| ChangeRecord::new(format!("gen/file-{index}.txt"), ChangeStatus::Added))
        .collect();
    let resolved = RecordingHookRunner::producing(reader.worktree_handle(), flood);
    let result = run_bump_resolved(
        &ws,
        root,
        &document,
        &writer,
        &reader,
        &provider_declaring(Version::new(0, 1, 0)),
        &resolved,
    );

    let error = result.expect_err("a worktree-flooding on-resolved hook fails closed");
    assert!(
        error.to_string().contains("new working-tree paths"),
        "the failure names the runaway-hook bound: {error}"
    );
    assert!(
        !writer
            .writes()
            .iter()
            .any(|write| matches!(write, VcsWrite::Stage { .. })),
        "an unbounded hook output is never staged: {:?}",
        writer.writes()
    );
}
