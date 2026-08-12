use std::collections::BTreeMap;
use std::path::Path;

use rskit_errors::AppResult;
use rskit_version::semver::Version;
use toven_model::{Module, ModuleKey, RepoPath};
use toven_ports::HookRunner;

use super::apply::{
    MemberReleaseShard, guard_member_trees, repo_for, restore_prepared_failure, shard_plan,
};
use super::hooks::{member_version_references, run_on_resolved_hooks};
use super::restore::restore_bump_prepared;
use crate::execution::{apply, version_sync};
use crate::{ReleasePlan, ReleaseStats};
use toven_core::federation::member_repo::MemberReleaseRepos;

/// Apply the standalone `bump` phase across member repos.
///
/// Runs only the version + changelog mutation half of a release, per member:
/// each member gets its own branch and clean-tree guardrail, its manifests are
/// rewritten and its configured changelog rolled, and the mutation is then
/// **staged** for a pull request. No commit, tag, push, publish, or hosted
/// Release is produced — creating the release commit/tag/push is the job of
/// `release tag` / `release publish` after the staged change merges. `date`
/// stamps a rolled changelog's versioned heading; `options.dry_run` reports the
/// planned mutation without writing.
///
/// The mutation runs in two phases so it stays undoable: every member's
/// manifests and changelog are written first, and any failure restores the
/// already-mutated members' working trees before surfacing. Only once all
/// members are prepared are the per-member stages created.
///
/// # Errors
/// Returns a typed error when a member repo port is missing, a clean-tree or
/// branch guardrail trips, or member mutation/changelog-roll/staging fails.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn release_bump_by_member(
    plan: &ReleasePlan,
    modules: &[Module],
    targets: &crate::ReleaseTargets,
    repos: &MemberReleaseRepos<'_>,
    date: &str,
    resolved_runner: &dyn HookRunner,
    options: crate::BumpOptions,
) -> AppResult<crate::BumpReport> {
    use crate::BumpReport;

    let mut report = BumpReport::empty(options);
    if plan.is_empty() {
        return Ok(report);
    }
    let module_by_ref: BTreeMap<ModuleKey, &Module> = modules
        .iter()
        .map(|module| (module.key(), module))
        .collect();
    let shards = shard_plan(plan, modules)?;
    let settings = shards
        .iter()
        .map(|shard| apply::reconcile_repo_settings(&shard.plan.entries))
        .collect::<AppResult<Vec<_>>>()?;
    guard_member_trees(&shards, &settings, repos)?;
    // Resolve every pre-mutation failure (missing target, non-rendering commit
    // template) before mutating any member.
    for (shard, settings) in shards.iter().zip(&settings) {
        apply::preflight_targets(&shard.plan, &module_by_ref, targets)?;
        apply::commit_message(&shard.plan, &module_by_ref, settings.commit_message())?;
    }

    if options.dry_run {
        report.modules = bump_module_outcomes(plan, &Vec::new());
        report.changelogs = would_roll_changelogs(plan);
        return Ok(report);
    }

    // Phase 1 (undoable): mutate manifests and roll the changelog for every
    // member. A failure restores each already-mutated member before surfacing.
    //
    // The authoritative post-bump version map is built once from the whole plan
    // so a version reference resolves cross-module (and cross-member) against a
    // single source of truth.
    let versions = version_sync::authoritative_versions(plan, &module_by_ref);
    let mut prepared: Vec<(&MemberReleaseShard, Vec<RepoPath>)> = Vec::new();
    for shard in &shards {
        let repo = repo_for(repos, shard.member.as_ref())?;
        match bump_prepare_member(
            &shard.plan,
            &module_by_ref,
            targets,
            repo.root(),
            date,
            &versions,
        ) {
            Ok(prepared_bump) => {
                report.modules.extend(bump_module_outcomes(
                    &shard.plan,
                    &prepared_bump.mutated_manifests,
                ));
                report.changelogs.extend(prepared_bump.rolled_changelogs);
                prepared.push((shard, prepared_bump.changed));
            }
            Err(error) => {
                // The current member may already be partially mutated (some
                // manifests, the changelog, or some reference files written
                // before the failure). Restore it as well, not only the earlier
                // completed shards, to honor the phase's undoable guarantee.
                let error = match repo.writer().restore_worktree() {
                    Ok(()) => error,
                    Err(restore) => restore_prepared_failure(error, &restore),
                };
                return Err(restore_bump_prepared(&prepared, repos, error));
            }
        }
    }

    // On-resolved seam: run the bump-scoped mid-mutation hooks now that every
    // member's version decision and native version-reference sync are done but
    // before anything is staged, handing each task the authoritative version
    // map. The task's edits join the staged set; a failure restores every
    // already-mutated member and stages nothing.
    run_on_resolved_hooks(plan, &module_by_ref, &mut prepared, repos, resolved_runner)?;

    // Phase 2: stage the mutation for a PR, per member. A member that rewrote
    // nothing (a tag-only ecosystem with no rolled changelog) has nothing to
    // stage, so `report.staged` reflects whether any member's mutation was
    // actually staged rather than the requested disposition.
    for (shard, changed) in &prepared {
        if changed.is_empty() {
            continue;
        }
        let repo = repo_for(repos, shard.member.as_ref())?;
        apply::stage_only(repo.writer(), changed)?;
        report.staged = true;
    }
    Ok(report)
}

/// One member's prepared `bump` mutation: the full staged path set, the
/// per-module rewritten manifest paths, and the rolled changelog paths.
struct PreparedBump {
    /// Every repo-relative path the commit or stage will pick up (manifests +
    /// rolled changelogs).
    changed: Vec<RepoPath>,
    /// Per-module rewritten manifest paths, for the report's module outcomes.
    mutated_manifests: Vec<(ModuleKey, Vec<RepoPath>)>,
    /// Repo-relative changelog paths that were rolled.
    rolled_changelogs: Vec<String>,
}

/// Mutate one member's manifests, roll its changelog, and sync its declared
/// version references, returning the staged path set, the per-module manifest
/// paths, and the rolled changelog paths.
///
/// `versions` is the whole-plan authoritative version map; the version-reference
/// sync resolves each member-declared file glob under `root` and rewrites its
/// pins against that one map, joining the rewritten paths to the staged set.
fn bump_prepare_member(
    plan: &ReleasePlan,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
    targets: &crate::ReleaseTargets,
    root: &Path,
    date: &str,
    versions: &BTreeMap<String, Version>,
) -> AppResult<PreparedBump> {
    use crate::execution::mutate;

    let mut stats = ReleaseStats::new(plan.entries.len());
    let mutated_manifests = mutate::mutate_manifests(plan, module_by_ref, targets, &mut stats)?;
    let rolled = mutate::roll_changelogs(plan, root, date)?;
    let mut changed = mutate::staged_paths(&mutated_manifests);
    changed.extend(rolled.iter().cloned());
    let references = member_version_references(plan);
    let synced = version_sync::sync_version_references(&references, versions, root)?;
    changed.extend(synced);
    let rolled_changelogs = rolled
        .iter()
        .map(|path| path.as_path().to_string_lossy().into_owned())
        .collect();
    Ok(PreparedBump {
        changed,
        mutated_manifests,
        rolled_changelogs,
    })
}

/// The repo-scoped union of the version references declared across a member's
/// plan entries, deduplicated so a reference inherited by every module is
fn bump_module_outcomes(
    plan: &ReleasePlan,
    mutated_manifests: &[(ModuleKey, Vec<RepoPath>)],
) -> Vec<crate::BumpModuleOutcome> {
    plan.entries
        .iter()
        .filter_map(|entry| {
            let new_version = entry.planned_version.clone()?;
            let manifests = mutated_manifests
                .iter()
                .find(|(module, _)| *module == entry.module)
                .map(|(_, paths)| {
                    paths
                        .iter()
                        .map(|path| path.as_path().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
            Some(crate::BumpModuleOutcome {
                module: entry.module.clone(),
                old_version: entry.current_version.clone(),
                new_version,
                manifests,
            })
        })
        .collect()
}

/// The distinct changelog paths a `bump` run would roll, in plan order — the
/// `--dry-run` preview of the changelog mutation.
fn would_roll_changelogs(plan: &ReleasePlan) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut paths = Vec::new();
    for entry in &plan.entries {
        if entry.changelog_roll
            && entry.planned_version.is_some()
            && seen.insert(entry.changelog_path.clone())
        {
            paths.push(entry.changelog_path.clone());
        }
    }
    paths
}
