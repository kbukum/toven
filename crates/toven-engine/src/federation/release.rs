//! Cross-repo release planning and per-member APPLY sharding.
//!
//! The release plan remains one federated plan, but history mutations are
//! scoped to each member repo: every member gets its own clean-tree guardrail,
//! release commit, tags, and optional push. Publishing is delayed until after
//! the member commits so registry work still follows the federated publish
//! order.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_model::{MemberId, Module, ModuleKey, RepoPath};
use toven_ports::{Artifact, VcsReader, VcsWriter};

use crate::release::apply;
use crate::release::publish;
use crate::release::{ReleaseApplyOptions, ReleasePlan, ReleaseStats};

/// One member repo's release VCS ports.
pub struct MemberReleaseRepo<'a> {
    member: Option<MemberId>,
    root: PathBuf,
    reader: &'a dyn VcsReader,
    writer: &'a dyn VcsWriter,
}

impl<'a> MemberReleaseRepo<'a> {
    /// Construct one member repo release port binding.
    ///
    /// `root` is the member's canonical repository root — the working directory
    /// forge commands (e.g. the hosted-release `gh` calls) run from, so a
    /// Release is cut against the repo whose tags this member pushed.
    #[must_use]
    pub fn new(
        member: Option<MemberId>,
        root: PathBuf,
        reader: &'a dyn VcsReader,
        writer: &'a dyn VcsWriter,
    ) -> Self {
        Self {
            member,
            root,
            reader,
            writer,
        }
    }

    /// The member this repo belongs to, or `None` for the degenerate project.
    #[must_use]
    pub const fn member(&self) -> Option<&MemberId> {
        self.member.as_ref()
    }

    /// The member's canonical repository root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Read-only VCS port for guardrails.
    #[must_use]
    pub const fn reader(&self) -> &dyn VcsReader {
        self.reader
    }

    /// Write VCS port for commit/tag/push/restore.
    #[must_use]
    pub const fn writer(&self) -> &dyn VcsWriter {
        self.writer
    }
}

/// Member repo release ports in declaration order.
pub struct MemberReleaseRepos<'a> {
    entries: Vec<MemberReleaseRepo<'a>>,
}

impl<'a> MemberReleaseRepos<'a> {
    /// Construct a member repo release port set.
    #[must_use]
    pub const fn new(entries: Vec<MemberReleaseRepo<'a>>) -> Self {
        Self { entries }
    }

    fn get(&self, member: Option<&MemberId>) -> Option<&MemberReleaseRepo<'a>> {
        self.entries
            .iter()
            .find(|entry| entry.member.as_ref() == member)
    }

    /// The canonical repository root for `member`, if it is a known member
    /// repo.
    #[must_use]
    pub fn root_for(&self, member: Option<&MemberId>) -> Option<&Path> {
        self.get(member).map(MemberReleaseRepo::root)
    }

    /// The read-only VCS port for `member`, if it is a known member repo.
    ///
    /// The reconcile pre-pass uses it to confirm that a published version's
    /// release tag exists before completing its missing hosted Release.
    #[must_use]
    pub fn reader_for(&self, member: Option<&MemberId>) -> Option<&dyn VcsReader> {
        self.get(member).map(MemberReleaseRepo::reader)
    }
}

/// Apply one federated release plan across member repos.
///
/// # Errors
/// Returns a typed error when a member repo port is missing, a clean-tree
/// guardrail trips, member mutation/packaging/commit/tag/push fails, or the
/// federated publish loop fails.
pub fn release_apply_by_member(
    plan: &ReleasePlan,
    modules: &[Module],
    targets: &crate::release::ReleaseTargets,
    repos: &MemberReleaseRepos<'_>,
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
    let mut preflights = Vec::with_capacity(shards.len());
    for (shard, settings) in shards.iter().zip(&settings) {
        // Target preflight: a member without a release target fails closed
        // before any member mutates.
        apply::preflight_targets(&shard.plan, &module_by_ref, targets)?;
        apply::commit_message(&shard.plan, &module_by_ref, settings.commit_message())?;
        // Immutable-tag preflight: a partial tag overlap fails closed before any
        // member mutates; an all-tags-exist member resumes.
        let repo = repo_for(repos, shard.member.as_ref())?;
        let preflight = apply::preflight_tags(&shard.plan, &module_by_ref, repo.reader())?;
        if matches!(preflight, apply::TagPreflight::Fresh) {
            apply::preflight_tag_signers(&shard.plan, repo.writer())?;
        }
        preflights.push(preflight);
    }
    if preflights
        .iter()
        .any(|preflight| matches!(preflight, apply::TagPreflight::Resume))
    {
        stats.resumed = true;
    }

    let mut artifacts = BTreeMap::new();
    let mut prepared = Vec::with_capacity(shards.len());
    let mut prepared_settings = Vec::with_capacity(shards.len());
    for ((shard, settings), preflight) in shards.iter().zip(&settings).zip(&preflights) {
        // An already-tagged member resumes: its commit, tags, and push already
        // exist on the remote and its manifest already carries the released
        // version, so mutation, commit, tag, and push are skipped. It still
        // packages any version the registry lacks so the shared publish tail can
        // complete a publish interrupted after tag/push; a fully-published
        // member packages nothing.
        if matches!(preflight, apply::TagPreflight::Resume) {
            match package_member_shard(shard, &module_by_ref, targets, repos, &mut stats) {
                Ok(member_artifacts) => artifacts.extend(member_artifacts),
                Err(error) => return Err(restore_prepared_or_error(&prepared, repos, error)),
            }
            continue;
        }
        match prepare_member_shard(shard, &module_by_ref, targets, repos, &mut stats) {
            Ok((member_changed, member_artifacts)) => {
                prepared.push((shard, member_changed, member_artifacts));
                prepared_settings.push(settings);
            }
            Err(error) => return Err(restore_prepared_or_error(&prepared, repos, error)),
        }
    }

    for ((shard, member_changed, member_artifacts), settings) in
        prepared.into_iter().zip(prepared_settings)
    {
        commit_member_shard(
            shard,
            &module_by_ref,
            repos,
            options,
            settings,
            &member_changed,
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

/// Apply the standalone `bump` phase across member repos.
///
/// Runs only the version + changelog mutation half of a release, per member:
/// each member gets its own branch and clean-tree guardrail, its manifests are
/// rewritten and its configured changelog rolled, and the mutation is then
/// either committed (the default) or staged for a pull request (`--no-commit`).
/// No tag, push, publish, or hosted Release is produced. `date` stamps a rolled
/// changelog's versioned heading; `options.dry_run` reports the planned mutation
/// without writing.
///
/// The mutation runs in two phases so it stays undoable up to the first commit:
/// every member's manifests and changelog are written first, and any failure
/// restores the already-mutated members' working trees before surfacing. Only
/// once all members are prepared are the per-member commits/stages created.
///
/// # Errors
/// Returns a typed error when a member repo port is missing, a clean-tree or
/// branch guardrail trips, or member mutation/changelog-roll/staging/commit
/// fails.
pub fn release_bump_by_member(
    plan: &ReleasePlan,
    modules: &[Module],
    targets: &crate::release::ReleaseTargets,
    repos: &MemberReleaseRepos<'_>,
    date: &str,
    options: &crate::release::BumpOptions,
) -> AppResult<crate::release::BumpReport> {
    use crate::release::BumpReport;

    let mut report = BumpReport::empty(*options);
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
    let mut prepared: Vec<(&MemberReleaseShard, Vec<RepoPath>)> = Vec::new();
    for shard in &shards {
        let repo = repo_for(repos, shard.member.as_ref())?;
        match bump_prepare_member(&shard.plan, &module_by_ref, targets, repo.root(), date) {
            Ok(prepared_bump) => {
                report.modules.extend(bump_module_outcomes(
                    &shard.plan,
                    &prepared_bump.mutated_manifests,
                ));
                report.changelogs.extend(prepared_bump.rolled_changelogs);
                prepared.push((shard, prepared_bump.changed));
            }
            Err(error) => return Err(restore_bump_prepared(&prepared, repos, error)),
        }
    }

    // Phase 2: create the release commit, or stage the mutation for a PR, per
    // member. A member that rewrote nothing (a tag-only ecosystem with no rolled
    // changelog) has nothing to commit or stage.
    for (shard, changed) in &prepared {
        if changed.is_empty() {
            continue;
        }
        let repo = repo_for(repos, shard.member.as_ref())?;
        if options.no_commit {
            apply::stage_only(repo.writer(), changed)?;
        } else {
            let settings = apply::reconcile_repo_settings(&shard.plan.entries)?;
            let message =
                apply::commit_message(&shard.plan, &module_by_ref, settings.commit_message())?;
            apply::stage_and_commit(repo.writer(), changed, &message)?;
        }
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

/// Mutate one member's manifests and roll its changelog, returning the staged
/// path set, the per-module manifest paths, and the rolled changelog paths.
fn bump_prepare_member(
    plan: &ReleasePlan,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
    targets: &crate::release::ReleaseTargets,
    root: &Path,
    date: &str,
) -> AppResult<PreparedBump> {
    use crate::release::mutate;

    let mut stats = ReleaseStats::new(plan.entries.len());
    let mutated_manifests = mutate::mutate_manifests(plan, module_by_ref, targets, &mut stats)?;
    let rolled = mutate::roll_changelogs(plan, root, date)?;
    let mut changed = mutate::staged_paths(&mutated_manifests);
    changed.extend(rolled.iter().cloned());
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

/// Restore every already-mutated member's working tree after a phase-1 failure,
/// mirroring [`restore_prepared_or_error`].
fn restore_bump_prepared(
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

/// Build per-module `bump` outcomes from a plan, mapping each planned entry to
/// its version transition and (when known) the manifest paths it rewrote.
fn bump_module_outcomes(
    plan: &ReleasePlan,
    mutated_manifests: &[(ModuleKey, Vec<RepoPath>)],
) -> Vec<crate::release::BumpModuleOutcome> {
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
            Some(crate::release::BumpModuleOutcome {
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

fn guard_member_trees(
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
    targets: &crate::release::ReleaseTargets,
    repos: &MemberReleaseRepos<'_>,
    stats: &mut ReleaseStats,
) -> AppResult<(Vec<RepoPath>, BTreeMap<ModuleKey, Artifact>)> {
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
    targets: &crate::release::ReleaseTargets,
    repos: &MemberReleaseRepos<'_>,
    stats: &mut ReleaseStats,
) -> AppResult<BTreeMap<ModuleKey, Artifact>> {
    repo_for(repos, shard.member.as_ref())?;
    apply::package_publishable(&shard.plan, module_by_ref, targets, stats)
}

fn commit_member_shard(
    shard: &MemberReleaseShard,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
    repos: &MemberReleaseRepos<'_>,
    options: &ReleaseApplyOptions,
    settings: &apply::RepoReleaseSettings,
    changed_paths: &[RepoPath],
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
    Ok(())
}

fn restore_prepared_or_error(
    prepared: &[(
        &MemberReleaseShard,
        Vec<RepoPath>,
        BTreeMap<ModuleKey, Artifact>,
    )],
    repos: &MemberReleaseRepos<'_>,
    error: AppError,
) -> AppError {
    for (shard, _, _) in prepared.iter().rev() {
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

fn restore_prepared_failure(error: AppError, restore: &AppError) -> AppError {
    AppError::new(
        ErrorCode::Internal,
        format!(
            "release prepare failed ({error}); additionally failed to restore a previously prepared member: {restore}"
        ),
    )
    .with_cause(error)
    .with_detail("restore_error", restore.to_string())
}

fn repo_for<'a>(
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
struct MemberReleaseShard {
    member: Option<MemberId>,
    plan: ReleasePlan,
}

fn shard_plan(plan: &ReleasePlan, modules: &[Module]) -> AppResult<Vec<MemberReleaseShard>> {
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

#[cfg(test)]
mod tests {

    use rskit_version::semver::Version;
    use toven_model::{EcosystemId, MemberId, Module, ModuleKey, ModuleRef, RepoPath};
    use toven_ports::ReleaseMutation;
    use toven_testkit::{FakeReleaseTarget, FakeVcsReader, FakeVcsWriter, ReleaseCall, VcsWrite};

    use super::{MemberReleaseRepo, MemberReleaseRepos, release_apply_by_member};
    use crate::release::{
        BumpPolicy, BumpReason, BumpSource, ChangelogEntry, PushPolicy, ReleaseApplyOptions,
        ReleaseEntry, ReleasePlan,
    };
    use toven_ports::BumpLevel;

    fn eid(id: &str) -> EcosystemId {
        EcosystemId::new(id).unwrap()
    }

    fn mref(name: &str) -> ModuleRef {
        ModuleRef::new(eid("rust"), name).unwrap()
    }

    fn member(name: &str) -> MemberId {
        MemberId::new(name).unwrap()
    }

    fn mkey(member: &str, name: &str) -> ModuleKey {
        ModuleKey::new(Some(self::member(member)), mref(name))
    }

    fn module(member: &str, name: &str) -> Module {
        let mut module = Module::new(
            mref(name),
            RepoPath::new(format!("repos/{member}/crates/{name}")).unwrap(),
        );
        module.member = Some(self::member(member));
        module
    }

    fn entry(member: &str, name: &str, version: Version, rank: usize) -> ReleaseEntry {
        ReleaseEntry {
            module: mkey(member, name),
            current_version: Version::new(0, 1, 0),
            planned_version: Some(version.clone()),
            planned_tag: Some(format!("rust/{name}@{version}")),
            level: BumpLevel::Patch,
            reason: BumpReason::Changed,
            winning_input: BumpSource::Default,
            cascade_origin: None,
            prerelease_channel: None,
            up_to_date: false,
            mutation: ReleaseMutation::version(version),
            publication: toven_ports::PublicationPolicy::Registry {
                registry: "crates-io".into(),
            },
            publish_needed: true,
            tag_format: None,
            tag_message: None,
            signer: None,
            commit_message: None,
            token_env: None,
            visibility: toven_ports::Visibility::Public,
            push: PushPolicy::BranchAndTags,
            remote: "origin".into(),
            branches: Vec::new(),
            topo_rank: rank,
            baseline: None,
            changelog: ChangelogEntry::new(mkey(member, name), "changed", Vec::new()),
            changelog_path: "CHANGELOG.md".into(),
            changelog_roll: false,
        }
    }

    fn targets(target: &FakeReleaseTarget) -> crate::release::ReleaseTargets {
        let mut map = crate::release::ReleaseTargets::new();
        // Every member in these fixtures exposes the same publishable `rust` target.
        for owner in ["core", "gateway"] {
            map.insert((Some(member(owner)), eid("rust")), Box::new(target.clone()));
        }
        map
    }

    fn targets_by_member(
        core: &FakeReleaseTarget,
        gateway: &FakeReleaseTarget,
    ) -> crate::release::ReleaseTargets {
        let mut map = crate::release::ReleaseTargets::new();
        map.insert((Some(member("core")), eid("rust")), Box::new(core.clone()));
        map.insert(
            (Some(member("gateway")), eid("rust")),
            Box::new(gateway.clone()),
        );
        map
    }

    #[test]
    fn an_all_member_tags_exist_release_resumes_without_git_mutation() {
        // Every member's planned tag already exists: the commits, tags, and
        // pushes happened on a prior attempt, so APPLY resumes across the
        // federation — no member mutates, commits, tags, or pushes — and the
        // already-published versions make the publish loop a clean no-op.
        let mut core_entry = entry("core", "shared", Version::new(0, 1, 1), 0);
        core_entry.publish_needed = false;
        core_entry.publication = toven_ports::PublicationPolicy::TagOnly;
        let mut gateway_entry = entry("gateway", "api", Version::new(0, 1, 1), 1);
        gateway_entry.publish_needed = false;
        gateway_entry.publication = toven_ports::PublicationPolicy::TagOnly;
        let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![core_entry, gateway_entry]);
        let modules = vec![module("core", "shared"), module("gateway", "api")];
        let target = FakeReleaseTarget::new();
        let core_reader = FakeVcsReader::new().with_tags(vec![toven_ports::TagRef::new(
            "rust/shared@0.1.1",
            toven_ports::Oid::new("deadbee"),
        )]);
        let gateway_reader = FakeVcsReader::new().with_tags(vec![toven_ports::TagRef::new(
            "rust/api@0.1.1",
            toven_ports::Oid::new("cafef00d"),
        )]);
        let core_writer = FakeVcsWriter::new();
        let gateway_writer = FakeVcsWriter::new();
        let repos = MemberReleaseRepos::new(vec![
            MemberReleaseRepo::new(
                Some(member("core")),
                std::path::PathBuf::from("/repos/core"),
                &core_reader,
                &core_writer,
            ),
            MemberReleaseRepo::new(
                Some(member("gateway")),
                std::path::PathBuf::from("/repos/gateway"),
                &gateway_reader,
                &gateway_writer,
            ),
        ]);

        let stats = release_apply_by_member(
            &plan,
            &modules,
            &targets(&target),
            &repos,
            &ReleaseApplyOptions::default(),
        )
        .expect("an already-tagged federation resumes rather than failing closed");

        assert!(stats.resumed, "the run is marked resumed");
        assert_eq!(stats.tagged_modules, 0);
        assert!(
            core_writer.writes().is_empty() && gateway_writer.writes().is_empty(),
            "no member may commit/tag/push on resume: core={:?} gateway={:?}",
            core_writer.writes(),
            gateway_writer.writes()
        );
        assert!(
            !target.calls().iter().any(|call| matches!(
                call,
                ReleaseCall::ApplyRelease { .. }
                    | ReleaseCall::Package(_)
                    | ReleaseCall::Publish(_)
            )),
            "no manifest mutation, packaging, or publish may happen on resume: {:?}",
            target.calls()
        );
    }

    #[test]
    fn a_partial_planned_tag_set_in_a_member_is_rejected_before_any_mutation() {
        // One member owns two distinct tags; only one exists — a partial overlap
        // within the member's own tag train is an interrupted or divergent
        // release, not a resume, and fails closed before any mutation.
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![
                entry("core", "shared", Version::new(0, 1, 1), 0),
                entry("core", "extra", Version::new(0, 1, 1), 1),
            ],
        );
        let modules = vec![module("core", "shared"), module("core", "extra")];
        let target = FakeReleaseTarget::new();
        let core_reader = FakeVcsReader::new().with_tags(vec![toven_ports::TagRef::new(
            "rust/shared@0.1.1",
            toven_ports::Oid::new("deadbee"),
        )]);
        let core_writer = FakeVcsWriter::new();
        let repos = MemberReleaseRepos::new(vec![MemberReleaseRepo::new(
            Some(member("core")),
            std::path::PathBuf::from("/repos/core"),
            &core_reader,
            &core_writer,
        )]);

        let error = release_apply_by_member(
            &plan,
            &modules,
            &targets(&target),
            &repos,
            &ReleaseApplyOptions::default(),
        )
        .expect_err("a partial tag overlap must fail closed before any mutation");

        let message = error.to_string();
        assert!(message.contains("immutable"), "{message}");
        assert!(message.contains("forward-fix"), "{message}");
        assert!(
            core_writer.writes().is_empty(),
            "no write may reach the member on a partial overlap"
        );
    }

    #[test]
    fn a_tag_failure_after_a_member_commit_carries_forward_only_recovery_guidance() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", "shared", Version::new(0, 1, 1), 0)],
        );
        let modules = vec![module("core", "shared")];
        let target = FakeReleaseTarget::new();
        let core_reader = FakeVcsReader::new();
        let core_writer = FakeVcsWriter::new().with_create_tag_failure("tag rejected");
        let repos = MemberReleaseRepos::new(vec![MemberReleaseRepo::new(
            Some(member("core")),
            std::path::PathBuf::from("/repos/core"),
            &core_reader,
            &core_writer,
        )]);

        let error = release_apply_by_member(
            &plan,
            &modules,
            &targets(&target),
            &repos,
            &ReleaseApplyOptions::default(),
        )
        .expect_err("a post-commit tag failure must surface recovery guidance");

        let message = error.to_string();
        assert!(message.contains("tagging"), "{message}");
        assert!(message.contains("tag rejected"), "{message}");
        assert!(message.contains("member 'core'"), "{message}");
        assert!(message.contains("toven release status"), "{message}");
        assert!(message.contains("forward fix"), "{message}");
        // The commit is past the rollback boundary: no worktree restore.
        assert!(
            core_writer
                .writes()
                .iter()
                .any(|write| matches!(write, VcsWrite::Commit { .. }))
        );
        assert!(
            !core_writer
                .writes()
                .iter()
                .any(|write| matches!(write, VcsWrite::RestoreWorktree))
        );
    }

    #[test]
    fn a_push_failure_after_the_member_commits_carries_forward_only_recovery_guidance() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", "shared", Version::new(0, 1, 1), 0)],
        );
        let modules = vec![module("core", "shared")];
        let target = FakeReleaseTarget::new();
        let core_reader = FakeVcsReader::new();
        let core_writer = FakeVcsWriter::new().with_push_failure("remote rejected");
        let repos = MemberReleaseRepos::new(vec![MemberReleaseRepo::new(
            Some(member("core")),
            std::path::PathBuf::from("/repos/core"),
            &core_reader,
            &core_writer,
        )]);

        let error = release_apply_by_member(
            &plan,
            &modules,
            &targets(&target),
            &repos,
            &ReleaseApplyOptions {
                no_push: false,
                ..Default::default()
            },
        )
        .expect_err("a post-commit push failure must surface recovery guidance");

        let message = error.to_string();
        assert!(message.contains("push"), "{message}");
        assert!(message.contains("remote rejected"), "{message}");
        assert!(message.contains("member 'core'"), "{message}");
        assert!(message.contains("toven release status"), "{message}");
        assert!(message.contains("forward fix"), "{message}");
        // The commit and tag are past the rollback boundary: no worktree
        // restore may be attempted for a post-commit push failure.
        assert!(
            core_writer
                .writes()
                .iter()
                .any(|write| matches!(write, VcsWrite::Commit { .. }))
        );
        assert!(
            !core_writer
                .writes()
                .iter()
                .any(|write| matches!(write, VcsWrite::RestoreWorktree))
        );
    }

    #[test]
    fn tags_only_member_push_proceeds_on_a_detached_head() {
        let mut shared = entry("core", "shared", Version::new(0, 1, 1), 0);
        shared.push = PushPolicy::TagsOnly;
        let plan = ReleasePlan::new(BumpPolicy::SemverCascade, vec![shared]);
        let modules = vec![module("core", "shared")];
        let target = FakeReleaseTarget::new();
        let core_reader = FakeVcsReader::new().with_detached_head();
        let core_writer = FakeVcsWriter::new();
        let repos = MemberReleaseRepos::new(vec![MemberReleaseRepo::new(
            Some(member("core")),
            std::path::PathBuf::from("/repos/core"),
            &core_reader,
            &core_writer,
        )]);

        release_apply_by_member(
            &plan,
            &modules,
            &targets(&target),
            &repos,
            &ReleaseApplyOptions {
                no_push: false,
                ..Default::default()
            },
        )
        .expect("a tags-only push does not require a checked-out branch");

        let (_, refspecs) = core_writer
            .writes()
            .into_iter()
            .find_map(|w| match w {
                VcsWrite::Push { remote, refspecs } => Some((remote, refspecs)),
                _ => None,
            })
            .expect("push recorded");
        assert_eq!(refspecs, vec!["refs/tags/rust/shared@0.1.1".to_string()]);
    }

    #[test]
    fn a_publish_failure_after_the_member_commits_carries_forward_only_recovery_guidance() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", "shared", Version::new(0, 1, 1), 0)],
        );
        let modules = vec![module("core", "shared")];
        let target = FakeReleaseTarget::new().with_publish_failure("registry unavailable");
        let core_reader = FakeVcsReader::new();
        let core_writer = FakeVcsWriter::new();
        let repos = MemberReleaseRepos::new(vec![MemberReleaseRepo::new(
            Some(member("core")),
            std::path::PathBuf::from("/repos/core"),
            &core_reader,
            &core_writer,
        )]);

        let error = release_apply_by_member(
            &plan,
            &modules,
            &targets(&target),
            &repos,
            &ReleaseApplyOptions::default(),
        )
        .expect_err("a post-commit publish failure must surface recovery guidance");

        let message = error.to_string();
        assert!(message.contains("publication"), "{message}");
        assert!(message.contains("registry unavailable"), "{message}");
        assert!(message.contains("toven release status"), "{message}");
        assert!(message.contains("forward fix"), "{message}");
        assert!(
            !core_writer
                .writes()
                .iter()
                .any(|write| matches!(write, VcsWrite::RestoreWorktree))
        );
    }

    #[test]
    fn release_apply_commits_per_member_and_publishes_in_federated_order() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![
                entry("core", "shared", Version::new(0, 1, 1), 0),
                entry("gateway", "api", Version::new(0, 1, 1), 1),
            ],
        );
        let modules = vec![module("core", "shared"), module("gateway", "api")];
        let target = FakeReleaseTarget::new();
        let core_reader = FakeVcsReader::new();
        let gateway_reader = FakeVcsReader::new();
        let core_writer = FakeVcsWriter::new().with_commit_oid("corecommit");
        let gateway_writer = FakeVcsWriter::new().with_commit_oid("gwcommit");
        let repos = MemberReleaseRepos::new(vec![
            MemberReleaseRepo::new(
                Some(member("core")),
                std::path::PathBuf::from("/repos/core"),
                &core_reader,
                &core_writer,
            ),
            MemberReleaseRepo::new(
                Some(member("gateway")),
                std::path::PathBuf::from("/repos/gateway"),
                &gateway_reader,
                &gateway_writer,
            ),
        ]);

        let stats = release_apply_by_member(
            &plan,
            &modules,
            &targets(&target),
            &repos,
            &ReleaseApplyOptions::default(),
        )
        .unwrap();

        assert_eq!(stats.mutated_modules, 2);
        assert_eq!(stats.tagged_modules, 2);
        assert_eq!(stats.published_modules, 2);
        assert!(matches!(
            &core_writer.writes()[0],
            VcsWrite::Commit { message, paths }
                if message == "release: rust/shared@0.1.1"
                    && paths == &vec!["repos/core/crates/shared".to_string()]
        ));
        assert!(matches!(
            &gateway_writer.writes()[0],
            VcsWrite::Commit { message, paths }
                if message == "release: rust/api@0.1.1"
                    && paths == &vec!["repos/gateway/crates/api".to_string()]
        ));
        assert!(matches!(
            &core_writer.writes()[1],
            VcsWrite::CreateTag { name, target_rev, .. }
                if name == "rust/shared@0.1.1" && target_rev == "corecommit"
        ));
        assert!(matches!(
            &gateway_writer.writes()[1],
            VcsWrite::CreateTag { name, target_rev, .. }
                if name == "rust/api@0.1.1" && target_rev == "gwcommit"
        ));

        let published = target
            .calls()
            .into_iter()
            .filter_map(|call| match call {
                ReleaseCall::Publish(module) => Some(module.to_string()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(published, vec!["rust:shared", "rust:api"]);
    }

    #[test]
    fn member_prepare_failure_restores_only_that_member() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![
                entry("core", "shared", Version::new(0, 1, 1), 0),
                entry("gateway", "api", Version::new(0, 1, 1), 1),
            ],
        );
        let modules = vec![module("core", "shared"), module("gateway", "api")];
        let target = FakeReleaseTarget::new().with_package_failure("package failed");
        let core_reader = FakeVcsReader::new();
        let gateway_reader = FakeVcsReader::new();
        let core_writer = FakeVcsWriter::new();
        let gateway_writer = FakeVcsWriter::new();
        let repos = MemberReleaseRepos::new(vec![
            MemberReleaseRepo::new(
                Some(member("core")),
                std::path::PathBuf::from("/repos/core"),
                &core_reader,
                &core_writer,
            ),
            MemberReleaseRepo::new(
                Some(member("gateway")),
                std::path::PathBuf::from("/repos/gateway"),
                &gateway_reader,
                &gateway_writer,
            ),
        ]);

        let error = release_apply_by_member(
            &plan,
            &modules,
            &targets(&target),
            &repos,
            &ReleaseApplyOptions::default(),
        )
        .unwrap_err();

        assert!(error.to_string().contains("package failed"));
        assert_eq!(core_writer.writes(), vec![VcsWrite::RestoreWorktree]);
        assert!(gateway_writer.writes().is_empty());
    }

    #[test]
    fn later_member_prepare_failure_restores_prepared_members_before_any_commit() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![
                entry("core", "shared", Version::new(0, 1, 1), 0),
                entry("gateway", "api", Version::new(0, 1, 1), 1),
            ],
        );
        let modules = vec![module("core", "shared"), module("gateway", "api")];
        let core_target = FakeReleaseTarget::new();
        let gateway_target =
            FakeReleaseTarget::new().with_package_failure("gateway package failed");
        let core_reader = FakeVcsReader::new();
        let gateway_reader = FakeVcsReader::new();
        let core_writer = FakeVcsWriter::new().with_commit_oid("corecommit");
        let gateway_writer = FakeVcsWriter::new();
        let repos = MemberReleaseRepos::new(vec![
            MemberReleaseRepo::new(
                Some(member("core")),
                std::path::PathBuf::from("/repos/core"),
                &core_reader,
                &core_writer,
            ),
            MemberReleaseRepo::new(
                Some(member("gateway")),
                std::path::PathBuf::from("/repos/gateway"),
                &gateway_reader,
                &gateway_writer,
            ),
        ]);

        let error = release_apply_by_member(
            &plan,
            &modules,
            &targets_by_member(&core_target, &gateway_target),
            &repos,
            &ReleaseApplyOptions::default(),
        )
        .unwrap_err();

        assert!(error.to_string().contains("gateway package failed"));
        assert_eq!(core_writer.writes(), vec![VcsWrite::RestoreWorktree]);
        assert_eq!(gateway_writer.writes(), vec![VcsWrite::RestoreWorktree]);
    }

    #[test]
    fn member_without_a_release_target_is_not_served_by_another_members_target() {
        // `core` publishes `rust`; `gateway` does not (its rust adapter is `publish =
        // false`, so it contributes no target). Keying targets by `(member, ecosystem)`
        // must not let `gateway`'s module borrow `core`'s target and get silently
        // released.
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![
                entry("core", "shared", Version::new(0, 1, 1), 0),
                entry("gateway", "api", Version::new(0, 1, 1), 1),
            ],
        );
        let modules = vec![module("core", "shared"), module("gateway", "api")];
        let mut targets = crate::release::ReleaseTargets::new();
        targets.insert(
            (Some(member("core")), eid("rust")),
            Box::new(FakeReleaseTarget::new()),
        );
        let core_reader = FakeVcsReader::new();
        let gateway_reader = FakeVcsReader::new();
        let core_writer = FakeVcsWriter::new().with_commit_oid("corecommit");
        let gateway_writer = FakeVcsWriter::new();
        let repos = MemberReleaseRepos::new(vec![
            MemberReleaseRepo::new(
                Some(member("core")),
                std::path::PathBuf::from("/repos/core"),
                &core_reader,
                &core_writer,
            ),
            MemberReleaseRepo::new(
                Some(member("gateway")),
                std::path::PathBuf::from("/repos/gateway"),
                &gateway_reader,
                &gateway_writer,
            ),
        ]);

        let error = release_apply_by_member(
            &plan,
            &modules,
            &targets,
            &repos,
            &ReleaseApplyOptions::default(),
        )
        .unwrap_err();

        assert!(error.to_string().contains("has no release target"));
        assert!(gateway_writer.writes().is_empty());
    }
}
