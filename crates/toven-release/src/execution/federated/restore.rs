use std::collections::BTreeSet;

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_model::RepoPath;
use toven_ports::ChangeStatus;

use super::apply::{MemberReleaseShard, repo_for, restore_prepared_failure};
use toven_core::federation::member_repo::{MemberReleaseRepo, MemberReleaseRepos};

/// Upper bound on the untracked paths a member's working tree may report at the
/// on-resolved seam before the bump treats the tree as pathological and fails
/// closed. Mirrors the source-tree walk bound so a runaway hook cannot force an
/// unbounded snapshot or deletion sweep.
pub(super) const MAX_UNTRACKED_PATHS: usize = 100_000;

/// The repo-relative untracked paths of every prepared member, index-aligned
/// with `prepared`, so the on-resolved failure path can tell hook-created files
/// apart from phase-1's own first-time output.
pub(super) fn untracked_snapshots(
    prepared: &[(&MemberReleaseShard, Vec<RepoPath>)],
    repos: &MemberReleaseRepos<'_>,
) -> AppResult<Vec<BTreeSet<std::path::PathBuf>>> {
    prepared
        .iter()
        .map(|(shard, _)| untracked_paths(repo_for(repos, shard.member.as_ref())?))
        .collect()
}

/// The repo-relative paths a member's working tree reports as untracked.
///
/// # Errors
/// Fails closed when the tree reports more than [`MAX_UNTRACKED_PATHS`]
/// untracked paths, so a runaway on-resolved hook cannot force an unbounded
/// snapshot or deletion sweep. The clean-tree guard proved the pre-bump tree
/// empty, so this bounds a hook's own first-time output, never expected state.
fn untracked_paths(repo: &MemberReleaseRepo) -> AppResult<BTreeSet<std::path::PathBuf>> {
    let mut paths = BTreeSet::new();
    for record in repo.reader().worktree_status()? {
        if record.status != ChangeStatus::Added {
            continue;
        }
        if paths.len() >= MAX_UNTRACKED_PATHS {
            return Err(runaway_hook_paths(repo));
        }
        paths.insert(record.path);
    }
    Ok(paths)
}

/// The fail-closed error for a member whose working tree carries more than
/// [`MAX_UNTRACKED_PATHS`] hook-produced paths, so a runaway on-resolved hook
/// cannot force an unbounded snapshot, deletion sweep, or staged set.
pub(super) fn runaway_hook_paths(repo: &MemberReleaseRepo) -> AppError {
    AppError::new(
        ErrorCode::Internal,
        format!(
            "an on-resolved hook left more than {MAX_UNTRACKED_PATHS} new working-tree paths in '{}'; \
             remove the stray files and retry the bump",
            repo.root().display()
        ),
    )
}

/// Abort a bump after an on-resolved hook fails: delete the untracked files the
/// hooks introduced in every member (paths untracked now but absent from
/// `before`), then roll every mutated member's tracked files back to `HEAD`, so
/// no partial mutation survives. Cleanup runs before the tracked restore because
/// `restore_worktree` intentionally leaves untracked files in place.
///
/// Cleanup is best-effort: a failure to enumerate or delete one member's
/// untracked files must never skip the tracked restore below, or the abort would
/// strand tracked manifest/version-reference mutations and break the atomic-abort
/// guarantee. The first cleanup failure is accumulated, cleanup continues for the
/// remaining members, and the tracked restore always runs; a surfaced cleanup
/// failure is folded into the returned error as a detail.
pub(super) fn abort_on_resolved(
    prepared: &[(&MemberReleaseShard, Vec<RepoPath>)],
    repos: &MemberReleaseRepos<'_>,
    before: &[BTreeSet<std::path::PathBuf>],
    error: AppError,
) -> AppError {
    let mut cleanup_failure: Option<AppError> = None;
    for ((shard, _), before) in prepared.iter().zip(before) {
        let repo = match repo_for(repos, shard.member.as_ref()) {
            Ok(repo) => repo,
            Err(cleanup) => {
                cleanup_failure.get_or_insert(cleanup);
                continue;
            }
        };
        if let Err(cleanup) = remove_new_untracked(repo, before) {
            cleanup_failure.get_or_insert(cleanup);
        }
    }
    let restored = restore_bump_prepared(prepared, repos, error);
    match cleanup_failure {
        Some(cleanup) => restored.with_detail("untracked_cleanup_error", cleanup.to_string()),
        None => restored,
    }
}

/// Delete the untracked files a member gained since `before` — the on-resolved
/// hooks' brand-new output — leaving pre-existing untracked paths untouched.
fn remove_new_untracked(
    repo: &MemberReleaseRepo,
    before: &BTreeSet<std::path::PathBuf>,
) -> AppResult<()> {
    for path in untracked_paths(repo)? {
        if before.contains(&path) {
            continue;
        }
        let absolute = rskit_fs::safe_join(repo.root(), &path).map_err(|error| {
            AppError::new(
                ErrorCode::Internal,
                "an on-resolved hook produced an untracked path outside the member repo",
            )
            .with_cause(error)
        })?;
        rskit_fs::sync_io::file::remove_if_exists(&absolute)?;
    }
    Ok(())
}

/// Restore every already-mutated member's working tree after a phase-1 failure,
/// mirroring [`super::apply::restore_prepared_or_error`].
pub(super) fn restore_bump_prepared(
    prepared: &[(&MemberReleaseShard, Vec<RepoPath>)],
    repos: &MemberReleaseRepos<'_>,
    error: AppError,
) -> AppError {
    for (shard, _) in prepared.iter().rev() {
        let repo = match repo_for(repos, shard.member.as_ref()) {
            Ok(repo) => repo,
            Err(restore) => return restore_prepared_failure(error, &restore),
        };
        if let Err(restore) = repo.writer().restore_worktree() {
            return restore_prepared_failure(error, &restore);
        }
    }
    error
}
