//! Real-repo coverage for the one rskit-git-backed adapter, [`RskitGitVcs`].
//!
//! Pure baseline/fan-out logic is unit-tested over fakes inside the crate;
//! these tests drive an actual temp git repo (via [`GitScenario`]) to prove the
//! composed git reads/writes — `changed_since`, `worktree_status`, `list_tags`,
//! `is_ignored`, `is_dirty`, `restore_worktree` — behave end to end.
//! Network-free and deterministic.

use std::path::{Path, PathBuf};

use rskit_fs::sync_io::file;
use toven_model::WorkspaceId;
use toven_ports::{BaselineSpec, ChangeRecord, ChangeStatus, VcsReader, VcsWriter};
use toven_testkit::{TestWorkspace, assert_ok, git::GitScenario};
use toven_vcs::{RskitGitVcs, VcsReaderSet};

fn find<'a>(records: &'a [ChangeRecord], path: &str) -> Option<&'a ChangeRecord> {
    records
        .iter()
        .find(|record| record.path.as_path() == Path::new(path))
}

#[test]
fn changed_since_reports_committed_diff_against_an_explicit_baseline() {
    let ws = TestWorkspace::new("vcs-changed-since");
    let scenario = assert_ok(GitScenario::init(ws.path()));
    assert_ok(scenario.commit_file("src/lib.rs", "fn a() {}\n", "c1"));
    assert_ok(scenario.tag("errors@1.0.0", "baseline"));
    assert_ok(scenario.commit_file("src/main.rs", "fn main() {}\n", "c2"));
    assert_ok(scenario.commit_file("src/lib.rs", "fn a() {} fn b() {}\n", "c3"));

    let vcs = assert_ok(RskitGitVcs::open(ws.path()));
    let changed = assert_ok(vcs.changed_since(&BaselineSpec::explicit("errors@1.0.0")));

    assert_eq!(
        find(&changed, "src/main.rs").map(|r| r.status),
        Some(ChangeStatus::Added)
    );
    assert_eq!(
        find(&changed, "src/lib.rs").map(|r| r.status),
        Some(ChangeStatus::Modified)
    );
}

#[test]
fn changed_since_merge_base_diffs_from_the_branch_point() {
    let ws = TestWorkspace::new("vcs-merge-base");
    let scenario = assert_ok(GitScenario::init(ws.path()));
    assert_ok(scenario.commit_file("base.rs", "0\n", "c0"));
    // A divergent commit on main that must NOT appear in a merge-base diff.
    assert_ok(scenario.commit_file("main-only.rs", "m\n", "main"));

    let vcs = assert_ok(RskitGitVcs::open(ws.path()));
    // merge-base(HEAD, HEAD) == HEAD, so nothing changed since the branch point.
    let changed = assert_ok(vcs.changed_since(&BaselineSpec::merge_base("HEAD")));

    assert!(
        changed.is_empty(),
        "merge-base against HEAD yields no diff: {changed:?}"
    );
}

#[test]
fn list_tags_filters_by_glob_pattern() {
    let ws = TestWorkspace::new("vcs-list-tags");
    let scenario = assert_ok(GitScenario::init(ws.path()));
    assert_ok(scenario.commit_file("a.rs", "0\n", "c0"));
    assert_ok(scenario.tag("errors@1.0.0", "annotated"));
    assert_ok(scenario.tag("config@2.0.0", "annotated"));

    let vcs = assert_ok(RskitGitVcs::open(ws.path()));

    let errors = assert_ok(vcs.list_tags(Some("errors@*")));
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].name, "errors@1.0.0");

    assert!(assert_ok(vcs.list_tags(Some("missing@*"))).is_empty());
    assert_eq!(assert_ok(vcs.list_tags(None)).len(), 2);
}

#[test]
fn worktree_status_and_is_dirty_track_uncommitted_changes() {
    let ws = TestWorkspace::new("vcs-worktree");
    let scenario = assert_ok(GitScenario::init(ws.path()));
    assert_ok(scenario.commit_file("tracked.rs", "0\n", "c0"));

    let vcs = assert_ok(RskitGitVcs::open(ws.path()));
    assert!(!assert_ok(vcs.is_dirty()), "clean tree after commit");
    assert!(assert_ok(vcs.worktree_status()).is_empty());

    assert_ok(scenario.write_file("untracked.rs", "new\n"));

    assert!(assert_ok(vcs.is_dirty()), "untracked file dirties the tree");
    let status = assert_ok(vcs.worktree_status());
    assert_eq!(
        find(&status, "untracked.rs").map(|r| r.status),
        Some(ChangeStatus::Added)
    );
}

#[test]
fn is_ignored_honours_gitignore() {
    let ws = TestWorkspace::new("vcs-ignore");
    let scenario = assert_ok(GitScenario::init(ws.path()));
    assert_ok(scenario.commit_file(".gitignore", "target/\n", "c0"));

    let vcs = assert_ok(RskitGitVcs::open(ws.path()));

    assert!(assert_ok(vcs.is_ignored(Path::new("target/debug/app"))));
    assert!(!assert_ok(vcs.is_ignored(Path::new("src/lib.rs"))));
}

#[test]
fn restore_worktree_rolls_tracked_files_back_to_head() {
    let ws = TestWorkspace::new("vcs-restore");
    let scenario = assert_ok(GitScenario::init(ws.path()));
    assert_ok(scenario.commit_file("manifest.toml", "version = \"1.0.0\"\n", "c0"));

    // Simulate a failed release apply that rewrote a tracked manifest.
    let manifest = ws.path().join("manifest.toml");
    assert_ok(file::write(&manifest, b"version = \"9.9.9\"\n"));

    let vcs = assert_ok(RskitGitVcs::open(ws.path()));
    assert!(
        assert_ok(vcs.is_dirty()),
        "rewritten manifest dirties the tree"
    );

    assert_ok(vcs.restore_worktree());

    assert!(
        !assert_ok(vcs.is_dirty()),
        "restore returns to a clean tree"
    );
    let restored = assert_ok(file::read_string_bounded(&manifest, 4096));
    assert_eq!(restored, "version = \"1.0.0\"\n");
}

#[test]
fn restore_worktree_tolerates_a_staged_new_file() {
    let ws = TestWorkspace::new("vcs-restore-staged-new");
    let scenario = assert_ok(GitScenario::init(ws.path()));
    assert_ok(scenario.commit_file("manifest.toml", "version = \"1.0.0\"\n", "c0"));

    // A first-time release artifact: a brand-new file, staged but never committed.
    assert_ok(scenario.write_file("CHANGELOG.md", "# 1.0.0\n"));
    assert_ok(scenario.stage_all());

    let vcs = assert_ok(RskitGitVcs::open(ws.path()));
    // Rollback must not error on the staged-new path (absent from HEAD).
    assert_ok(vcs.restore_worktree());
}

#[test]
fn commit_and_create_tag_advance_history() {
    let ws = TestWorkspace::new("vcs-write");
    let scenario = assert_ok(GitScenario::init(ws.path()));
    assert_ok(scenario.commit_file("a.rs", "0\n", "c0"));
    // A release-style mutation left in the working tree, unstaged: commit must
    // stage exactly this path so the release commit carries the bump and the
    // tree ends clean (the empty-commit / dangling-bump regression).
    assert_ok(scenario.write_file("a.rs", "1\n"));

    let vcs = assert_ok(RskitGitVcs::open(ws.path()));
    let oid = assert_ok(vcs.commit("release: bump", &["a.rs"]));
    assert!(!oid.as_str().is_empty());
    assert!(
        !assert_ok(vcs.is_dirty()),
        "commit stages the mutated path, leaving a clean tree"
    );

    assert_ok(vcs.create_tag("pkg@1.0.0", "HEAD", Some("release"), None));
    let tags = assert_ok(vcs.list_tags(Some("pkg@*")));
    assert_eq!(tags.len(), 1);
    assert_eq!(tags[0].name, "pkg@1.0.0");
}

#[test]
fn reader_set_dedups_two_workspaces_under_one_repo() {
    let ws = TestWorkspace::new("vcs-reader-set");
    let scenario = assert_ok(GitScenario::init(ws.path()));
    assert_ok(scenario.commit_file("apps/web/main.rs", "0\n", "web"));
    assert_ok(scenario.commit_file("apps/api/lib.rs", "0\n", "api"));

    let members = vec![
        (
            assert_ok(WorkspaceId::new("web")),
            ws.path().join("apps/web"),
        ),
        (
            assert_ok(WorkspaceId::new("api")),
            ws.path().join("apps/api"),
        ),
    ];
    let set = assert_ok(VcsReaderSet::open(&members, &[]));

    assert_eq!(set.groups().len(), 1, "both workspaces share one repo");
    let group = &set.groups()[0];
    assert_eq!(group.members().len(), 2);
    let prefixes = group
        .members()
        .iter()
        .map(|m| m.prefix().to_path_buf())
        .collect::<Vec<_>>();
    assert!(prefixes.contains(&PathBuf::from("apps/web")));
    assert!(prefixes.contains(&PathBuf::from("apps/api")));
}
