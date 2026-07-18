//! Cross-repo release planning and per-member APPLY sharding.
//!
//! The release plan remains one federated plan, but history mutations are scoped
//! to each member repo: every member gets its own clean-tree guardrail, release
//! commit, tags, and optional push. Publishing is delayed until after the member
//! commits so registry work still follows the federated publish order.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_model::{MemberId, Module, ModuleKey};
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
    /// forge commands (e.g. the hosted-release `gh` calls) run from, so a Release
    /// is cut against the repo whose tags this member pushed.
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

    /// The canonical repository root for `member`, if it is a known member repo.
    #[must_use]
    pub fn root_for(&self, member: Option<&MemberId>) -> Option<&Path> {
        self.get(member).map(MemberReleaseRepo::root)
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
    guard_member_trees(&shards, repos, options)?;

    let mut prepared = Vec::with_capacity(shards.len());
    for shard in &shards {
        match prepare_member_shard(shard, &module_by_ref, targets, repos, &mut stats) {
            Ok(member_artifacts) => prepared.push((shard, member_artifacts)),
            Err(error) => return Err(restore_prepared_or_error(&prepared, repos, error)),
        }
    }

    let mut artifacts = BTreeMap::new();
    for (shard, member_artifacts) in prepared {
        commit_member_shard(shard, &module_by_ref, targets, repos, options, &mut stats)?;
        artifacts.extend(member_artifacts);
    }

    let items = apply::publish_items(plan, &module_by_ref, targets, &artifacts)?;
    publish::run(&items, options.retry_budget, &mut stats)?;
    Ok(stats)
}

fn guard_member_trees(
    shards: &[MemberReleaseShard],
    repos: &MemberReleaseRepos<'_>,
    options: &ReleaseApplyOptions,
) -> AppResult<()> {
    for shard in shards {
        let repo = repo_for(repos, shard.member.as_ref())?;
        apply::guard_clean_tree(repo.reader(), options)?;
    }
    Ok(())
}

fn prepare_member_shard(
    shard: &MemberReleaseShard,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
    targets: &crate::release::ReleaseTargets,
    repos: &MemberReleaseRepos<'_>,
    stats: &mut ReleaseStats,
) -> AppResult<BTreeMap<ModuleKey, Artifact>> {
    let repo = repo_for(repos, shard.member.as_ref())?;
    apply::prepare(&shard.plan, module_by_ref, targets, stats)
        .map_err(|error| apply::restore_or_precommit_error(repo.writer(), "prepare", error))
}

fn commit_member_shard(
    shard: &MemberReleaseShard,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
    targets: &crate::release::ReleaseTargets,
    repos: &MemberReleaseRepos<'_>,
    options: &ReleaseApplyOptions,
    stats: &mut ReleaseStats,
) -> AppResult<()> {
    let repo = repo_for(repos, shard.member.as_ref())?;
    let message = apply::commit_message(&shard.plan, module_by_ref, targets)?;
    let commit = match repo.writer().commit(&message) {
        Ok(commit) => commit,
        Err(error) => {
            return Err(apply::restore_or_precommit_error(
                repo.writer(),
                "commit",
                error,
            ));
        }
    };
    for entry in &shard.plan.entries {
        if let Some(version) = &entry.planned_version {
            let module = module_by_ref.get(&entry.module).copied().ok_or_else(|| {
                AppError::invalid_input(
                    "release.modules",
                    format!("unknown module '{}'", entry.module),
                )
            })?;
            let target = targets
                .get(&(module.member.clone(), module.id.ecosystem.clone()))
                .map(Box::as_ref)
                .ok_or_else(|| {
                    AppError::invalid_input(
                        "release.target",
                        format!("module '{}' has no release target", module.key()),
                    )
                })?;
            let scheme = target.tag_scheme(module, entry.tag_format.as_deref())?;
            let name = crate::release::tag::format(&scheme, version);
            repo.writer()
                .create_tag(&name, commit.as_str(), Some(&message))?;
            stats.tagged_modules += 1;
        }
    }
    if options.push {
        repo.writer()
            .push(&apply::push_refspecs(&shard.plan, module_by_ref, targets)?)?;
    }
    Ok(())
}

fn restore_prepared_or_error(
    prepared: &[(&MemberReleaseShard, BTreeMap<ModuleKey, Artifact>)],
    repos: &MemberReleaseRepos<'_>,
    error: AppError,
) -> AppError {
    for (shard, _) in prepared.iter().rev() {
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
        BumpPolicy, BumpReason, BumpSource, ChangelogEntry, ReleaseApplyOptions, ReleaseEntry,
        ReleasePlan,
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
            level: BumpLevel::Patch,
            reason: BumpReason::Changed,
            winning_input: BumpSource::Default,
            cascade_origin: None,
            prerelease_channel: None,
            up_to_date: false,
            mutation: ReleaseMutation::version(version),
            publish_needed: true,
            tag_format: None,
            topo_rank: rank,
            baseline: None,
            changelog: ChangelogEntry::new(mkey(member, name), "changed", Vec::new()),
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
            MemberReleaseRepo::new(Some(member("core")), std::path::PathBuf::from("/repos/core"), &core_reader, &core_writer),
            MemberReleaseRepo::new(Some(member("gateway")), std::path::PathBuf::from("/repos/gateway"), &gateway_reader, &gateway_writer),
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
            VcsWrite::Commit(message) if message == "release: rust/shared@0.1.1"
        ));
        assert!(matches!(
            &gateway_writer.writes()[0],
            VcsWrite::Commit(message) if message == "release: rust/api@0.1.1"
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
            MemberReleaseRepo::new(Some(member("core")), std::path::PathBuf::from("/repos/core"), &core_reader, &core_writer),
            MemberReleaseRepo::new(Some(member("gateway")), std::path::PathBuf::from("/repos/gateway"), &gateway_reader, &gateway_writer),
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
            MemberReleaseRepo::new(Some(member("core")), std::path::PathBuf::from("/repos/core"), &core_reader, &core_writer),
            MemberReleaseRepo::new(Some(member("gateway")), std::path::PathBuf::from("/repos/gateway"), &gateway_reader, &gateway_writer),
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
        // `core` publishes `rust`; `gateway` does not (its rust adapter is
        // `publish = false`, so it contributes no target). Keying targets by
        // `(member, ecosystem)` must not let `gateway`'s module borrow `core`'s
        // target and get silently released.
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
            MemberReleaseRepo::new(Some(member("core")), std::path::PathBuf::from("/repos/core"), &core_reader, &core_writer),
            MemberReleaseRepo::new(Some(member("gateway")), std::path::PathBuf::from("/repos/gateway"), &gateway_reader, &gateway_writer),
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
        assert_eq!(gateway_writer.writes(), vec![VcsWrite::RestoreWorktree]);
    }
}
