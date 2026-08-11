//! `commits_since` — the changelog range walk composed from rskit-git
//! primitives.
//!
//! Toven does not need a bespoke git range primitive: rskit-git already exposes
//! a path-filtered [`log`](rskit_git::LogReader::log) from `HEAD` and an
//! [`is_ancestor`](rskit_git::LogReader::is_ancestor) reachability check. The
//! `since..HEAD` range is the path-filtered log with every commit already
//! contained in `since` (the prior release ref) dropped — the same set
//! `git log since..HEAD -- <path>` yields, forge-agnostically and from git data
//! alone.

use std::path::Path;

use rskit_errors::AppResult;
use rskit_git::{Inspector, LogOptions, LogReader, Repo};
use toven_ports::CommitSummary;

/// Walk the commits reachable from `HEAD` but not from `since`, newest first,
/// optionally restricted to those touching `path_prefix`.
///
/// `since = None` walks the full path-filtered history (a first release).
/// Otherwise every commit already reachable from `since` (the prior release
/// ref) is skipped, yielding the same set `git log since..HEAD -- <path>`
/// produces. Released commits are *skipped rather than used as a stop point*:
/// rskit-git sorts the log by time within topological order, so a commit that
/// branched off before the baseline but merged in after it can appear later in
/// the walk than the baseline itself — stopping at the baseline would silently
/// drop those in-range commits. The per-commit reachability check is a
/// release-time cost bounded by the repository's history.
pub(super) fn commits_since(
    repo: &Repo,
    since: Option<&str>,
    path_prefix: Option<&Path>,
) -> AppResult<Vec<CommitSummary>> {
    let opts = LogOptions {
        path_filter: path_prefix.map(|path| path.to_string_lossy().into_owned()),
        ..LogOptions::default()
    };
    let baseline_oid = match since {
        Some(reference) => Some(repo.rev_parse(reference)?.to_string()),
        None => None,
    };

    let mut summaries = Vec::new();
    for commit in repo.log(Some(&opts))? {
        let oid = commit.oid.to_string();
        if let (Some(reference), Some(baseline)) = (since, baseline_oid.as_deref()) {
            // Skip the baseline commit and everything it already contains: the
            // half-open `since..HEAD` range excludes everything reachable from
            // `since`. Skipping (not stopping) keeps in-range commits that a
            // time-ordered walk emits after the baseline — e.g. a feature branch
            // cut before the baseline and merged after it.
            if oid == baseline || repo.is_ancestor(&oid, reference)? {
                continue;
            }
        }
        summaries.push(to_commit_summary(&commit, &oid));
    }
    Ok(summaries)
}

/// Map an rskit-git [`Commit`](rskit_git::Commit) onto the ports'
/// [`CommitSummary`], abbreviating the id and splitting subject from body.
fn to_commit_summary(commit: &rskit_git::Commit, oid: &str) -> CommitSummary {
    let short = oid.get(..12).unwrap_or(oid);
    CommitSummary::from_message(short, &commit.message)
        .with_author(&commit.author.name, &commit.author.email)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use toven_testkit::{TestWorkspace, git::GitScenario};

    use super::commits_since;
    use crate::RskitGitVcs;

    #[test]
    fn walks_only_commits_after_the_baseline() {
        let workspace = TestWorkspace::new("commits-since-range");
        let scenario = GitScenario::init(workspace.path()).expect("git init");
        scenario
            .commit_file("README.md", "one", "chore: initial")
            .expect("c1");
        scenario.tag_lightweight("v0.1.0").expect("tag baseline");
        scenario
            .commit_file("src/lib.rs", "two", "feat: add lib")
            .expect("c2");
        scenario
            .commit_file("src/lib.rs", "three", "fix: patch lib")
            .expect("c3");

        let vcs = RskitGitVcs::open(workspace.path()).expect("open");
        let commits = commits_since(vcs.repo(), Some("v0.1.0"), None).expect("commits");

        let subjects: Vec<&str> = commits.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects, vec!["fix: patch lib", "feat: add lib"]);
    }

    #[test]
    fn none_baseline_walks_full_history() {
        let workspace = TestWorkspace::new("commits-since-initial");
        let scenario = GitScenario::init(workspace.path()).expect("git init");
        scenario
            .commit_file("README.md", "one", "chore: initial")
            .expect("c1");
        scenario
            .commit_file("src/lib.rs", "two", "feat: add lib")
            .expect("c2");

        let vcs = RskitGitVcs::open(workspace.path()).expect("open");
        let commits = commits_since(vcs.repo(), None, None).expect("commits");

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].subject, "feat: add lib");
    }

    #[test]
    fn path_filter_scopes_to_touched_paths() {
        let workspace = TestWorkspace::new("commits-since-path");
        let scenario = GitScenario::init(workspace.path()).expect("git init");
        scenario
            .commit_file("README.md", "root", "chore: initial")
            .expect("c1");
        scenario.tag_lightweight("v0.1.0").expect("tag");
        scenario
            .commit_file("crates/a/src/lib.rs", "a", "feat: crate a")
            .expect("c2");
        scenario
            .commit_file("crates/b/src/lib.rs", "b", "feat: crate b")
            .expect("c3");

        let vcs = RskitGitVcs::open(workspace.path()).expect("open");
        let commits =
            commits_since(vcs.repo(), Some("v0.1.0"), Some(Path::new("crates/a"))).expect("scoped");

        let subjects: Vec<&str> = commits.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects, vec!["feat: crate a"]);
    }

    #[test]
    fn keeps_range_commits_a_feature_branch_merged_after_the_baseline() {
        // Non-linear history where a time-ordered walk emits the baseline before
        // some in-range commits: feat forks before the baseline (older-dated
        // commits) but merges in after it. Stopping at the baseline would drop
        // the feature commits; skipping keeps them.
        use std::time::{Duration, UNIX_EPOCH};
        let t = |secs: u64| UNIX_EPOCH + Duration::from_secs(secs);

        let workspace = TestWorkspace::new("commits-since-merge-topology");
        let scenario = GitScenario::init(workspace.path()).expect("git init");
        scenario.write_file("README.md", "root").expect("write");
        scenario
            .commit_all_pinned("chore: root", t(1_000))
            .expect("root");

        scenario.branch_and_checkout("feat").expect("feat branch");
        scenario.write_file("feat.rs", "c").expect("write c");
        scenario
            .commit_all_pinned("feat: earlier feature", t(2_000))
            .expect("feat c");
        scenario.write_file("feat.rs", "d").expect("write d");
        scenario
            .commit_all_pinned("fix: earlier fix", t(3_000))
            .expect("feat d");

        scenario.checkout("main").expect("back to main");
        scenario.write_file("main.rs", "b").expect("write b");
        scenario
            .commit_all_pinned("chore: baseline", t(5_000))
            .expect("baseline");
        scenario.tag_lightweight("v0.1.0").expect("tag baseline");
        scenario
            .merge_no_ff("feat", "chore: merge feat")
            .expect("merge");

        let vcs = RskitGitVcs::open(workspace.path()).expect("open");
        let commits = commits_since(vcs.repo(), Some("v0.1.0"), None).expect("commits");

        let mut subjects: Vec<&str> = commits.iter().map(|c| c.subject.as_str()).collect();
        subjects.sort_unstable();
        // The merge commit plus both feature-branch commits — none omitted — and
        // never the baseline commit itself.
        assert_eq!(
            subjects,
            vec![
                "chore: merge feat",
                "feat: earlier feature",
                "fix: earlier fix"
            ]
        );
    }

    #[test]
    fn author_identity_is_captured() {
        let workspace = TestWorkspace::new("commits-since-author");
        let scenario = GitScenario::init(workspace.path()).expect("git init");
        scenario
            .commit_file("src/lib.rs", "one", "feat: add lib")
            .expect("c1");

        let vcs = RskitGitVcs::open(workspace.path()).expect("open");
        let commits = commits_since(vcs.repo(), None, None).expect("commits");

        assert!(!commits[0].author_name.is_empty());
        assert!(commits[0].author_email.contains('@'));
    }
}
