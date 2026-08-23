use std::collections::{BTreeMap, BTreeSet};

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_version::semver::Version;
use toven_model::{Module, ModuleKey, RepoPath};
use toven_ports::{HookInvocation, HookRunner};

use super::apply::{MemberReleaseShard, repo_for};
use super::restore::{
    MAX_UNTRACKED_PATHS, abort_on_resolved, restore_bump_prepared, runaway_hook_paths,
    untracked_snapshots,
};
use crate::ReleasePlan;
use toven_core::federation::member_repo::MemberReleaseRepos;

/// The repo-scoped union of the version references declared across a member's
/// plan entries, deduplicated so a reference inherited by every module is
/// applied once.
pub(super) fn member_version_references(
    plan: &ReleasePlan,
) -> Vec<toven_ports::VersionReferenceConfig> {
    let mut seen = BTreeSet::new();
    let mut references = Vec::new();
    for entry in &plan.entries {
        for reference in &entry.version_references {
            if seen.insert((reference.files.clone(), reference.pattern.clone())) {
                references.push(reference.clone());
            }
        }
    }
    references
}

/// The repo-scoped union of the `on-resolved` task references declared across a
/// member's plan entries, deduplicated and in first-seen order so a reference
/// inherited by every module runs once.
fn member_on_resolved(plan: &ReleasePlan) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut references = Vec::new();
    for entry in &plan.entries {
        for reference in &entry.on_resolved {
            if seen.insert(reference.clone()) {
                references.push(reference.clone());
            }
        }
    }
    references
}

/// Run the bump `on-resolved` hooks (if any) after every member's version
/// decision and native version-reference sync but before staging, then join the
/// working-tree edits each task produced to the corresponding member's staged
/// set.
///
/// The authoritative version map is materialized once to a generated file
/// **outside** the repo (so the file never itself becomes a staged untracked
/// change) and its path is handed to each task argv-first. A failing task
/// restores every already-mutated member and surfaces the failure, staging
/// nothing.
pub(super) fn run_on_resolved_hooks(
    plan: &ReleasePlan,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
    prepared: &mut [(&MemberReleaseShard, Vec<RepoPath>)],
    repos: &MemberReleaseRepos<'_>,
    runner: &dyn HookRunner,
) -> AppResult<()> {
    let references = member_on_resolved(plan);
    if references.is_empty() {
        return Ok(());
    }
    // The temp-dir creation and version-map write run after every member has
    // already been mutated, so a failure here must undo those tracked edits —
    // route both through the same phase-1 restore as the snapshot read below.
    let versions = resolved_version_map(plan, module_by_ref);
    let scratch = match rskit_fs::TempDir::new() {
        Ok(scratch) => scratch,
        Err(error) => return Err(restore_bump_prepared(prepared, repos, error)),
    };
    let map_path = match write_version_map(&scratch, &versions) {
        Ok(map_path) => map_path,
        Err(error) => return Err(restore_bump_prepared(prepared, repos, error)),
    };
    // Snapshot each member's untracked set before the hooks run. The clean-tree
    // guard proved the pre-bump tree empty, so this captures only phase-1's own
    // first-time output — letting the failure path delete exactly the untracked
    // files the hooks introduce and leave no partial state behind. A snapshot
    // read that itself fails happens before any hook has run, so only phase-1's
    // tracked mutations need rolling back.
    let before = match untracked_snapshots(prepared, repos) {
        Ok(before) => before,
        Err(error) => return Err(restore_bump_prepared(prepared, repos, error)),
    };
    for reference in &references {
        if let Err(error) = runner.run_hook(
            HookInvocation::OnResolved {
                version_map: &map_path,
            },
            reference,
        ) {
            return Err(abort_on_resolved(prepared, repos, &before, error));
        }
    }
    // Joining the hook edits can itself fail (a status read or path parse) after
    // the hooks already mutated tracked files and produced untracked output;
    // route that through the same abort so no partial state survives.
    if let Err(error) = join_hook_edits(prepared, repos) {
        return Err(abort_on_resolved(prepared, repos, &before, error));
    }
    Ok(())
}

/// Join each member's hook-produced working-tree edits into its staged path set,
/// index-aligned with `prepared`: any path the tree now reports that the member
/// did not already stage is a hook edit and joins the staged set.
fn join_hook_edits(
    prepared: &mut [(&MemberReleaseShard, Vec<RepoPath>)],
    repos: &MemberReleaseRepos<'_>,
) -> AppResult<()> {
    for (shard, changed) in prepared.iter_mut() {
        let repo = repo_for(repos, shard.member.as_ref())?;
        let mut seen: BTreeSet<RepoPath> = changed.iter().cloned().collect();
        // Bound the hook-produced additions like the failure-path snapshot does,
        // so a runaway hook cannot force an unbounded staged set. (Enumeration
        // itself is still bounded only by the VCS status port; a truly streaming
        // bound would require that port to accept a limit.)
        let mut added = 0usize;
        for record in repo.reader().worktree_status()? {
            let path = RepoPath::new(record.path)?;
            if seen.insert(path.clone()) {
                if added >= MAX_UNTRACKED_PATHS {
                    return Err(runaway_hook_paths(repo));
                }
                added += 1;
                changed.push(path);
            }
        }
    }
    Ok(())
}

/// Build the collision-free `key → post-bump version` map handed to the bump
/// `on-resolved` hooks.
///
/// Delegates to [`version_sync::authoritative_versions`], which builds the
/// canonical member-qualified and unambiguous alias mapping across all members.
pub(super) fn resolved_version_map(
    plan: &ReleasePlan,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
) -> BTreeMap<String, Version> {
    crate::execution::version_sync::authoritative_versions(plan, module_by_ref)
}

/// Materialize the resolved `key → version` map as a stable JSON object at
/// `versions.json` under `scratch`, returning its absolute path for the
/// argv-first hand-off. Keys are each module's canonical member-qualified
/// identity plus any unambiguous package/`ecosystem:name`/bare-name alias (see
/// [`resolved_version_map`]); values are the post-bump version strings.
/// `BTreeMap` iteration keeps the object deterministically ordered.
///
/// # Errors
/// Propagates a serialization or temp-file write failure.
fn write_version_map(
    scratch: &rskit_fs::TempDir,
    versions: &BTreeMap<String, Version>,
) -> AppResult<std::path::PathBuf> {
    let rendered: BTreeMap<&String, String> = versions
        .iter()
        .map(|(key, version)| (key, version.to_string()))
        .collect();
    let json = serde_json::to_vec_pretty(&rendered).map_err(|error| {
        AppError::new(
            ErrorCode::Internal,
            "the bump on-resolved version map could not be serialized to JSON",
        )
        .with_cause(error)
    })?;
    scratch.write_file("versions.json", &json)
}
