use std::collections::BTreeMap;

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_model::{MemberId, Module, ModuleKey, RepoPath};
use toven_ports::Artifact;

use crate::execution::apply;
use crate::execution::mutate::MutatedManifests;
use crate::hosting::publish;
use crate::{ReleaseApplyOptions, ReleasePlan, ReleaseStats};
pub(super) use toven_core::federation::member_repo::{MemberReleaseRepo, MemberReleaseRepos};

/// One prepared member awaiting commit: its shard, the repo-relative paths its
/// mutations rewrote, the artifacts packaged for publish, and the per-module
/// mutated manifests used to project the module's *commit* event.
type PreparedShard<'a> = (
    &'a MemberReleaseShard,
    Vec<RepoPath>,
    BTreeMap<ModuleKey, Artifact>,
    MutatedManifests,
);

/// Apply one federated release plan across member repos.
///
/// # Errors
/// Returns a typed error when a member repo port is missing, a clean-tree
/// guardrail trips, member mutation/packaging/commit/tag/push fails, or the
/// federated publish loop fails.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn release_apply_by_member(
    plan: &ReleasePlan,
    modules: &[Module],
    targets: &crate::ReleaseTargets,
    repos: &MemberReleaseRepos<'_>,
    reporter: &mut dyn toven_ports::Reporter,
    options: &ReleaseApplyOptions,
) -> AppResult<ReleaseStats> {
    let mut stats = ReleaseStats::new(plan.entries.len());
    if plan.is_empty() {
        return Ok(stats);
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
    // Classify each member shard's apply mode. A maintainer-owned member runs
    // against a tag/Release a human already created: its tags are an input to
    // verify (fail closed if absent), never a mutation, so it skips the
    // tag-preflight/mutation/commit path entirely and only publishes. Same-member
    // entrypoint homogeneity is enforced in `reconcile_repo_settings`, so a whole
    // shard is either maintainer-owned or Toven-owned.
    let mut modes = Vec::with_capacity(shards.len());
    for (shard, settings) in shards.iter().zip(&settings) {
        // Target preflight: a member without a release target fails closed
        // before any member mutates or publishes.
        apply::preflight_targets(&shard.plan, &module_by_ref, targets)?;
        let repo = repo_for(repos, shard.member.as_ref())?;
        if settings.entrypoint().is_maintainer_owned() {
            // The maintainer's tags must already exist for the planned version;
            // Toven never creates or moves them.
            apply::verify_maintainer_tags(&shard.plan, &module_by_ref, repo.reader())?;
            modes.push(ShardApply::Maintainer);
            continue;
        }
        apply::commit_message(&shard.plan, &module_by_ref, settings.commit_message())?;
        // Immutable-tag preflight: a partial tag overlap fails closed before any
        // member mutates; an all-tags-exist member resumes.
        let preflight = apply::preflight_tags(&shard.plan, &module_by_ref, repo.reader())?;
        if matches!(preflight, apply::TagPreflight::Fresh) {
            apply::preflight_tag_signers(&shard.plan, repo.writer())?;
        }
        modes.push(ShardApply::Toven(preflight));
    }
    if modes
        .iter()
        .any(|mode| matches!(mode, ShardApply::Toven(apply::TagPreflight::Resume)))
    {
        stats.resumed = true;
    }

    let mut artifacts = BTreeMap::new();
    let mut prepared = Vec::with_capacity(shards.len());
    let mut prepared_settings = Vec::with_capacity(shards.len());
    for ((shard, settings), mode) in shards.iter().zip(&settings).zip(&modes) {
        match mode {
            // Neither a maintainer-owned member nor an already-tagged
            // (`Resume`) member mutates: both only package the versions the
            // registry still lacks to feed the shared publish tail — no manifest
            // mutation, commit, tag, or push. A maintainer-owned member's
            // manifest already carries the released version and its tags already
            // exist as a human-created input; a `Resume` member's commit, tags,
            // and push already exist on the remote from a prior attempt (a
            // fully-published member packages nothing).
            ShardApply::Maintainer | ShardApply::Toven(apply::TagPreflight::Resume) => {
                match package_member_shard(shard, &module_by_ref, targets, repos, &mut stats) {
                    Ok(member_artifacts) => artifacts.extend(member_artifacts),
                    Err(error) => return Err(restore_prepared_or_error(&prepared, repos, error)),
                }
            }
            ShardApply::Toven(apply::TagPreflight::Fresh) => {
                match prepare_member_shard(shard, &module_by_ref, targets, repos, &mut stats) {
                    Ok((member_changed, member_artifacts, member_mutated)) => {
                        prepared.push((shard, member_changed, member_artifacts, member_mutated));
                        prepared_settings.push(settings);
                    }
                    Err(error) => return Err(restore_prepared_or_error(&prepared, repos, error)),
                }
            }
        }
    }

    for ((shard, member_changed, member_artifacts, member_mutated), settings) in
        prepared.into_iter().zip(prepared_settings)
    {
        commit_member_shard(
            shard,
            &module_by_ref,
            repos,
            options,
            settings,
            &member_changed,
            &member_mutated,
            reporter,
            &mut stats,
        )?;
        artifacts.extend(member_artifacts);
    }

    if options.publish {
        let items = apply::publish_items(plan, &module_by_ref, targets, &artifacts)?;
        publish::run(&items, options.retry_budget, &mut stats).map_err(|error| {
            apply::forward_recovery_error(
                "the release commits and tags completed",
                "publication",
                error,
            )
        })?;
    }
    Ok(stats)
}
pub(super) fn guard_member_trees(
    shards: &[MemberReleaseShard],
    settings: &[apply::RepoReleaseSettings],
    repos: &MemberReleaseRepos<'_>,
) -> AppResult<()> {
    for (shard, settings) in shards.iter().zip(settings) {
        let repo = repo_for(repos, shard.member.as_ref())?;
        apply::guard_release_branch(repo.reader(), settings.branches())?;
        apply::guard_clean_tree(repo.reader())?;
    }
    Ok(())
}

fn prepare_member_shard(
    shard: &MemberReleaseShard,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
    targets: &crate::ReleaseTargets,
    repos: &MemberReleaseRepos<'_>,
    stats: &mut ReleaseStats,
) -> AppResult<(
    Vec<RepoPath>,
    BTreeMap<ModuleKey, Artifact>,
    MutatedManifests,
)> {
    let repo = repo_for(repos, shard.member.as_ref())?;
    apply::prepare(&shard.plan, module_by_ref, targets, stats)
        .map_err(|error| apply::restore_or_precommit_error(repo.writer(), "prepare", error))
}

/// Package a resumed member's still-publishable versions without mutating its
/// manifest. The member's release commit, tags, and push already exist and its
/// manifest already carries the released version, so only packaging is needed to
/// feed the shared publish tail for a publish interrupted after tag/push; a
/// fully-published member packages nothing.
fn package_member_shard(
    shard: &MemberReleaseShard,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
    targets: &crate::ReleaseTargets,
    repos: &MemberReleaseRepos<'_>,
    stats: &mut ReleaseStats,
) -> AppResult<BTreeMap<ModuleKey, Artifact>> {
    repo_for(repos, shard.member.as_ref())?;
    apply::package_publishable(&shard.plan, module_by_ref, targets, stats)
}

#[allow(clippy::too_many_arguments)]
fn commit_member_shard(
    shard: &MemberReleaseShard,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
    repos: &MemberReleaseRepos<'_>,
    options: &ReleaseApplyOptions,
    settings: &apply::RepoReleaseSettings,
    changed_paths: &[RepoPath],
    mutated: &[(ModuleKey, Vec<RepoPath>)],
    reporter: &mut dyn toven_ports::Reporter,
    stats: &mut ReleaseStats,
) -> AppResult<()> {
    let repo = repo_for(repos, shard.member.as_ref())?;
    let message = apply::commit_message(&shard.plan, module_by_ref, settings.commit_message())?;
    // A member that rewrote manifests stages exactly those paths and creates its
    // release commit; a mutation-free member (a Go tag-only cut, which rewrites
    // no `go.mod`) tags its existing `HEAD` instead of fabricating an empty
    // commit. Staging or commit failure leaves the member's mutations undoable.
    let created_commit = !changed_paths.is_empty();
    let commit = if created_commit {
        match apply::stage_and_commit(repo.writer(), changed_paths, &message) {
            Ok(commit) => commit,
            Err(error) => {
                return Err(apply::restore_or_precommit_error(
                    repo.writer(),
                    "commit",
                    error,
                ));
            }
        }
    } else {
        repo.reader().rev_parse("HEAD")?
    };
    // Post-commit phase for this member (no rollback): tag, optionally push. A
    // failure here cannot undo the member's release refs — it surfaces with
    // forward-only recovery guidance naming the member.
    let member = shard.member.as_ref();
    let committed = || {
        let anchor = commit.as_str();
        let name = member.map_or("<root>", MemberId::as_str);
        if created_commit {
            format!("the release commit {anchor} for member '{name}' was created")
        } else {
            format!("release tags for member '{name}' were applied to existing commit {anchor}")
        }
    };
    apply::tag_releases(&shard.plan, module_by_ref, repo.writer(), &commit, stats)
        .map_err(|error| apply::forward_recovery_error(&committed(), "tagging", error))?;
    if settings.pushes(options) {
        // Every push-phase step — resolving the branch (only when the branch
        // itself is pushed, so a tags-only push never needs one), computing
        // refspecs, and the push itself — runs after this member's commit and
        // tags exist, so any failure carries forward-only recovery guidance
        // naming the member.
        let push = || -> AppResult<()> {
            let branch = settings
                .pushes_branch()
                .then(|| repo.reader().current_branch())
                .transpose()?;
            let refspecs = apply::push_refspecs(&shard.plan, branch.as_deref())?;
            if refspecs.is_empty() {
                return Ok(());
            }
            repo.writer().push(settings.remote(), &refspecs)
        };
        push().map_err(|error| apply::forward_recovery_error(&committed(), "push", error))?;
    }
    // Post-commit, no rollback: this member's release commit and tags have
    // landed, so stream one commit event per genuinely-cut module in plan order.
    // A dependency-floor-only module (no planned version) is never tagged and so
    // emits nothing.
    emit_member_committed(reporter, shard, mutated)?;
    Ok(())
}

/// Emit one `ModuleReleaseStaged` commit event per module whose release commit
/// and tag have just landed for this member.
///
/// Draws the version and planned tag from the shard's plan entries and the
/// rewritten manifest paths from the member's mutation set, in deterministic
/// plan order. A module with no own-version bump receives no tag, but a
/// dependency-floor-only module still committed its rewritten manifest, so it
/// emits a staged event carrying no `new_version` and no tag rather than being
/// dropped. `run` rolls no changelog (that is the `bump` phase's job), so the
/// commit event carries none.
fn emit_member_committed(
    reporter: &mut dyn toven_ports::Reporter,
    shard: &MemberReleaseShard,
    mutated: &[(ModuleKey, Vec<RepoPath>)],
) -> AppResult<()> {
    for entry in &shard.plan.entries {
        let manifests = mutated
            .iter()
            .find(|(module, _)| *module == entry.module)
            .map(|(_, paths)| {
                paths
                    .iter()
                    .map(|path| path.as_path().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        reporter.emit(&crate::stream::staged_event(
            &entry.module,
            entry.planned_version.as_ref(),
            manifests,
            None,
            entry.planned_tag.clone(),
        ))?;
    }
    Ok(())
}

pub(super) fn restore_prepared_or_error(
    prepared: &[PreparedShard<'_>],
    repos: &MemberReleaseRepos<'_>,
    error: AppError,
) -> AppError {
    for (shard, _, _, _) in prepared.iter().rev() {
        let repo = match repo_for(repos, shard.member.as_ref()) {
            Ok(repo) => repo,
            Err(restore) => {
                return restore_prepared_failure(error, &restore);
            }
        };
        if let Err(restore) = repo.writer().restore_worktree() {
            return restore_prepared_failure(error, &restore);
        }
    }
    error
}

pub(super) fn restore_prepared_failure(error: AppError, restore: &AppError) -> AppError {
    AppError::new(
        ErrorCode::Internal,
        format!(
            "release prepare failed ({error}); additionally failed to restore a previously prepared member: {restore}"
        ),
    )
    .with_cause(error)
    .with_detail("restore_error", restore.to_string())
}

pub(super) fn repo_for<'a>(
    repos: &'a MemberReleaseRepos<'a>,
    member: Option<&MemberId>,
) -> AppResult<&'a MemberReleaseRepo<'a>> {
    repos.get(member).ok_or_else(|| {
        AppError::new(
            ErrorCode::Internal,
            format!(
                "release repo ports are missing for member '{}'",
                member.map_or("<root>", MemberId::as_str)
            ),
        )
    })
}

#[derive(Debug)]
pub(super) struct MemberReleaseShard {
    pub(super) member: Option<MemberId>,
    pub(super) plan: ReleasePlan,
}

/// How a single member shard is applied: the maintainer-owned publish-only path
/// (tags are a verified input, nothing mutates), or the Toven-owned path carrying
/// its immutable-tag preflight verdict (`Fresh` mutates/commits/tags/pushes;
/// `Resume` completes an interrupted publish without re-running the git phase).
#[derive(Debug)]
enum ShardApply {
    Maintainer,
    Toven(apply::TagPreflight),
}

pub(super) fn shard_plan(
    plan: &ReleasePlan,
    modules: &[Module],
) -> AppResult<Vec<MemberReleaseShard>> {
    let module_members = modules
        .iter()
        .map(|module| (module.key(), module.member.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut order: Vec<Option<MemberId>> = Vec::new();
    let mut entries = BTreeMap::<Option<MemberId>, Vec<_>>::new();
    for entry in &plan.entries {
        let member = module_members.get(&entry.module).cloned().ok_or_else(|| {
            AppError::invalid_input(
                "release.modules",
                format!("unknown module '{}'", entry.module),
            )
        })?;
        if !entries.contains_key(&member) {
            order.push(member.clone());
        }
        entries.entry(member).or_default().push(entry.clone());
    }
    Ok(order
        .into_iter()
        .filter_map(|member| {
            entries.remove(&member).map(|entries| MemberReleaseShard {
                member,
                plan: ReleasePlan::new(plan.policy, entries),
            })
        })
        .collect())
}
