//! The release APPLY transaction: clean-tree guardrail, manifest mutation,
//! packaging, a single release commit, per-module tagging, optional push, and
//! the bounded publish loop.
//!
//! The transaction has a hard commit-success boundary. Everything before a
//! successful commit (mutation + packaging + attempted commit) is undoable: any
//! failure restores the working tree and creates no commit or tag. Tags,
//! optional push, and the publish loop run after that boundary and are **not**
//! rolled back — a publish failure surfaces as a typed error and the operator
//! resumes, relying on registry idempotency.

use std::collections::BTreeMap;

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_model::{Module, ModuleKey};
use toven_ports::{Artifact, ReleaseTarget, VcsReader, VcsWriter};

use super::publish::{self, PublishItem};
use super::{ReleasePlan, ReleaseStats, tag};

/// Default rate-limit retry budget for the publish loop.
const DEFAULT_RETRY_BUDGET: usize = 5;

/// Runtime options for the release APPLY transaction.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ReleaseApplyOptions {
    /// Bypass the clean-tree guardrail (commit/tag a dirty working tree).
    pub allow_dirty: bool,
    /// Push the release commit and tags after tagging.
    pub push: bool,
    /// Publish the packaged artifacts to the registry after tagging. When
    /// false, the pipeline stops after commit/tag/push (the `release tag`
    /// surface).
    pub publish: bool,
    /// Maximum rate-limit retries per module in the publish loop.
    pub retry_budget: usize,
}

impl Default for ReleaseApplyOptions {
    fn default() -> Self {
        Self {
            allow_dirty: false,
            push: false,
            publish: true,
            retry_budget: DEFAULT_RETRY_BUDGET,
        }
    }
}

/// Execute a [`ReleasePlan`] against the ecosystem release targets and the VCS.
///
/// `modules` must contain every module referenced by the plan; `targets` must
/// hold a release target for every ecosystem in the plan.
///
/// # Errors
/// Returns a typed error when the clean-tree guardrail trips, a module/target
/// is missing, a pre-commit mutation/package/commit fails (after restoring the
/// working tree), a VCS tag/push fails, or the publish loop exhausts its retry
/// budget.
pub fn release_apply(
    plan: &ReleasePlan,
    modules: &[Module],
    targets: &super::ReleaseTargets,
    reader: &dyn VcsReader,
    writer: &dyn VcsWriter,
    options: &ReleaseApplyOptions,
) -> AppResult<ReleaseStats> {
    let mut stats = ReleaseStats::new(plan.entries.len());
    if plan.is_empty() {
        return Ok(stats);
    }

    // The clean-tree guardrail runs before any mutation.
    guard_clean_tree(reader, options)?;

    let module_by_ref: BTreeMap<ModuleKey, &Module> = modules
        .iter()
        .map(|module| (module.key(), module))
        .collect();

    // Pre-commit phase (undoable): apply mutations, then package every module.
    let artifacts = match prepare(plan, &module_by_ref, targets, &mut stats) {
        Ok(artifacts) => artifacts,
        Err(error) => return Err(restore_or_precommit_error(writer, "prepare", error)),
    };

    // Commit boundary: if commit itself fails, no history was created yet, so the
    // pre-commit working tree mutations are still undoable.
    let message = commit_message(plan, &module_by_ref, targets)?;
    let commit = match writer.commit(&message) {
        Ok(commit) => commit,
        Err(error) => return Err(restore_or_precommit_error(writer, "commit", error)),
    };

    // Post-commit phase (no rollback): tag, optionally push, publish.
    for entry in &plan.entries {
        if let Some(version) = &entry.planned_version {
            let module = module_for(&module_by_ref, &entry.module)?;
            let target = target_for(targets, module)?;
            let scheme = target.tag_scheme(module, entry.tag_format.as_deref())?;
            let name = tag::format(&scheme, version);
            writer.create_tag(&name, commit.as_str(), Some(&message))?;
            stats.tagged_modules += 1;
        }
    }
    if options.push {
        writer.push(&push_refspecs(plan, &module_by_ref, targets)?)?;
    }

    if options.publish {
        let items = publish_items(plan, &module_by_ref, targets, &artifacts)?;
        publish::run(&items, options.retry_budget, &mut stats)?;
    }

    Ok(stats)
}

#[allow(clippy::redundant_pub_crate)]
pub(crate) fn restore_or_precommit_error(
    writer: &dyn VcsWriter,
    phase: &str,
    error: AppError,
) -> AppError {
    match writer.restore_worktree() {
        Ok(()) => error,
        Err(restore) => AppError::new(
            ErrorCode::Internal,
            format!(
                "release {phase} failed ({error}); additionally failed to restore worktree: {restore}"
            ),
        )
        .with_cause(error)
        .with_detail("restore_error", restore.to_string()),
    }
}

/// Reject a dirty working tree unless `--allow-dirty` was requested.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn guard_clean_tree(
    reader: &dyn VcsReader,
    options: &ReleaseApplyOptions,
) -> AppResult<()> {
    if options.allow_dirty {
        return Ok(());
    }
    let status = reader.worktree_status()?;
    if status.is_empty() {
        return Ok(());
    }
    Err(AppError::invalid_input(
        "release.worktree",
        format!(
            "the working tree has {} uncommitted change(s); commit, stash, or pass --allow-dirty",
            status.len()
        ),
    ))
}

/// Apply every mutation and package every module, returning the artifacts keyed
/// by module. Runs entirely before the commit so the caller can restore the
/// working tree on failure.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn prepare(
    plan: &ReleasePlan,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
    targets: &super::ReleaseTargets,
    stats: &mut ReleaseStats,
) -> AppResult<BTreeMap<ModuleKey, Artifact>> {
    for entry in &plan.entries {
        let module = module_for(module_by_ref, &entry.module)?;
        let target = target_for(targets, module)?;
        target.apply_release(module, &entry.mutation)?;
        stats.mutated_modules += 1;
    }

    let mut artifacts = BTreeMap::new();
    for entry in &plan.entries {
        let module = module_for(module_by_ref, &entry.module)?;
        let target = target_for(targets, module)?;
        artifacts.insert(entry.module.clone(), target.package(module)?);
        stats.packaged_artifacts += 1;
    }
    Ok(artifacts)
}

/// Resolve the ordered publish items, skipping entries that need no publish.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn publish_items<'a>(
    plan: &'a ReleasePlan,
    module_by_ref: &BTreeMap<ModuleKey, &'a Module>,
    targets: &'a super::ReleaseTargets,
    artifacts: &'a BTreeMap<ModuleKey, Artifact>,
) -> AppResult<Vec<PublishItem<'a>>> {
    let mut items = Vec::new();
    for entry in &plan.entries {
        if !entry.publish_needed {
            continue;
        }
        let module = module_for(module_by_ref, &entry.module)?;
        // A publish-needed entry is always packaged with a planned version in the
        // pre-commit phase; a missing one is an internal inconsistency, not a skip.
        let (Some(version), Some(artifact)) =
            (entry.planned_version.as_ref(), artifacts.get(&entry.module))
        else {
            return Err(AppError::new(
                ErrorCode::Internal,
                format!(
                    "module '{}' is marked publish-needed but has no planned version or artifact",
                    entry.module
                ),
            ));
        };
        items.push(PublishItem {
            module,
            target: target_for(targets, module)?,
            artifact,
            version,
        });
    }
    Ok(items)
}

fn module_for<'a>(
    module_by_ref: &BTreeMap<ModuleKey, &'a Module>,
    reference: &ModuleKey,
) -> AppResult<&'a Module> {
    module_by_ref.get(reference).copied().ok_or_else(|| {
        AppError::invalid_input("release.modules", format!("unknown module '{reference}'"))
    })
}

fn target_for<'a>(
    targets: &'a super::ReleaseTargets,
    module: &Module,
) -> AppResult<&'a dyn ReleaseTarget> {
    targets
        .get(&(module.member.clone(), module.id.ecosystem.clone()))
        .map(Box::as_ref)
        .ok_or_else(|| {
            AppError::invalid_input(
                "release.target",
                format!("module '{}' has no release target", module.key()),
            )
        })
}

/// Build the single release commit message from the released module versions.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn commit_message(
    plan: &ReleasePlan,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
    targets: &super::ReleaseTargets,
) -> AppResult<String> {
    let mut released = Vec::new();
    for entry in &plan.entries {
        if let Some(version) = &entry.planned_version {
            let module = module_for(module_by_ref, &entry.module)?;
            let target = target_for(targets, module)?;
            let scheme = target.tag_scheme(module, entry.tag_format.as_deref())?;
            released.push(tag::format(&scheme, version));
        }
    }
    Ok(format!("release: {}", released.join(", ")))
}

/// Refspecs pushed after tagging: the release commit plus every release tag.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn push_refspecs(
    plan: &ReleasePlan,
    module_by_ref: &BTreeMap<ModuleKey, &Module>,
    targets: &super::ReleaseTargets,
) -> AppResult<Vec<String>> {
    let mut refspecs = vec!["HEAD".to_string()];
    for entry in &plan.entries {
        if let Some(version) = &entry.planned_version {
            let module = module_for(module_by_ref, &entry.module)?;
            let target = target_for(targets, module)?;
            let scheme = target.tag_scheme(module, entry.tag_format.as_deref())?;
            refspecs.push(format!("refs/tags/{}", tag::format(&scheme, version)));
        }
    }
    Ok(refspecs)
}

#[cfg(test)]
mod tests {

    use rskit_errors::ErrorCode;
    use rskit_version::semver::Version;
    use toven_model::{EcosystemId, Module, ModuleKey, ModuleRef, RepoPath};
    use toven_ports::{ChangeRecord, ChangeStatus, PublishOutcome, ReleaseMutation, TagScheme};
    use toven_testkit::{FakeReleaseTarget, FakeVcsReader, FakeVcsWriter, ReleaseCall, VcsWrite};

    use super::{ReleaseApplyOptions, release_apply};
    use crate::release::{
        BumpPolicy, BumpReason, BumpSource, ChangelogEntry, ReleaseEntry, ReleasePlan,
    };
    use toven_ports::BumpLevel;

    fn mref(name: &str) -> ModuleRef {
        ModuleRef::new(EcosystemId::new("rust").unwrap(), name).unwrap()
    }

    fn mkey(name: &str) -> ModuleKey {
        ModuleKey::bare(mref(name))
    }

    fn module(name: &str) -> Module {
        let mut module = Module::new(mref(name), RepoPath::new(format!("crates/{name}")).unwrap());
        module.manifest = Some(RepoPath::new(format!("crates/{name}/Cargo.toml")).unwrap());
        module
    }

    fn entry(name: &str, version: Version, publish_needed: bool, rank: usize) -> ReleaseEntry {
        ReleaseEntry {
            module: mkey(name),
            current_version: Version::new(0, 1, 0),
            planned_version: Some(version.clone()),
            level: BumpLevel::Patch,
            reason: BumpReason::Changed,
            winning_input: BumpSource::Default,
            cascade_origin: None,
            prerelease_channel: None,
            up_to_date: false,
            mutation: ReleaseMutation::version(version),
            publish_needed,
            tag_format: None,
            topo_rank: rank,
            baseline: None,
            changelog: ChangelogEntry::new(mkey(name), "changed", Vec::new()),
        }
    }

    fn targets(pairs: Vec<(&str, FakeReleaseTarget)>) -> super::super::ReleaseTargets {
        // All fixtures use a single single-repo `rust` ecosystem.
        let mut map = super::super::ReleaseTargets::new();
        let (_, target) = pairs.into_iter().next().expect("at least one target");
        map.insert((None, EcosystemId::new("rust").unwrap()), Box::new(target));
        map
    }

    fn dirty() -> FakeVcsReader {
        FakeVcsReader::new()
            .with_worktree_status(vec![ChangeRecord::new("a.rs", ChangeStatus::Modified)])
    }

    #[test]
    fn applies_mutations_commits_tags_and_publishes_in_order() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![
                entry("core", Version::new(0, 1, 1), true, 0),
                entry("app", Version::new(0, 1, 1), true, 1),
            ],
        );
        let modules = vec![module("core"), module("app")];
        let target = FakeReleaseTarget::new();
        let writer = FakeVcsWriter::new().with_commit_oid("c0ffee");

        let stats = release_apply(
            &plan,
            &modules,
            &targets(vec![("core", target.clone())]),
            &FakeVcsReader::new(),
            &writer,
            &ReleaseApplyOptions::default(),
        )
        .expect("release apply");

        assert_eq!(stats.mutated_modules, 2);
        assert_eq!(stats.packaged_artifacts, 2);
        assert_eq!(stats.tagged_modules, 2);
        assert_eq!(stats.published_modules, 2);
        assert_eq!(stats.skipped_published_modules, 0);

        let recorded = writer.writes();
        assert_eq!(
            recorded[0],
            VcsWrite::Commit("release: rust/core@0.1.1, rust/app@0.1.1".into())
        );
        assert!(matches!(
            &recorded[1],
            VcsWrite::CreateTag { name, target_rev, .. } if name == "rust/core@0.1.1" && target_rev == "c0ffee"
        ));
        assert!(
            matches!(&recorded[2], VcsWrite::CreateTag { name, .. } if name == "rust/app@0.1.1")
        );
        assert!(!recorded.iter().any(|w| matches!(w, VcsWrite::Push(_))));

        // Publish happens after the commit/tag writes (apply -> package -> publish).
        let calls = target.calls();
        assert!(
            calls
                .iter()
                .filter(|c| matches!(c, ReleaseCall::ApplyRelease { .. }))
                .count()
                == 2
        );
        assert!(
            calls
                .iter()
                .filter(|c| matches!(c, ReleaseCall::Publish(_)))
                .count()
                == 2
        );
    }

    #[test]
    fn push_emits_commit_and_tag_refspecs() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(1, 0, 0), true, 0)],
        );
        let writer = FakeVcsWriter::new();

        release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", FakeReleaseTarget::new())]),
            &FakeVcsReader::new(),
            &writer,
            &ReleaseApplyOptions {
                push: true,
                ..Default::default()
            },
        )
        .expect("release apply with push");

        let push = writer
            .writes()
            .into_iter()
            .find_map(|w| match w {
                VcsWrite::Push(refspecs) => Some(refspecs),
                _ => None,
            })
            .expect("push recorded");
        assert_eq!(
            push,
            vec!["HEAD".to_string(), "refs/tags/rust/core@1.0.0".to_string()]
        );
    }

    #[test]
    fn tag_only_run_commits_and_tags_without_publishing() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(0, 1, 1), true, 0)],
        );
        let target = FakeReleaseTarget::new();
        let writer = FakeVcsWriter::new().with_commit_oid("c0ffee");

        let stats = release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", target.clone())]),
            &FakeVcsReader::new(),
            &writer,
            &ReleaseApplyOptions {
                publish: false,
                ..Default::default()
            },
        )
        .expect("tag-only release apply");

        assert_eq!(stats.tagged_modules, 1);
        assert_eq!(stats.published_modules, 0);
        assert!(
            writer.writes().iter().any(
                |w| matches!(w, VcsWrite::CreateTag { name, .. } if name == "rust/core@0.1.1")
            )
        );
        assert!(
            !target
                .calls()
                .iter()
                .any(|c| matches!(c, ReleaseCall::Publish(_))),
            "tag-only run must not publish"
        );
    }

    #[test]
    fn mixed_ecosystem_umbrella_tags_each_member_with_its_own_scheme() {
        // A Rust crate (crates.io tag grammar) and a Go module (path-based git tag
        // grammar) release over the one topological order, each carrying its own
        // target-owned tag scheme.
        let go_ref = ModuleRef::new(EcosystemId::new("go").unwrap(), "cache-redis").unwrap();
        let go_key = ModuleKey::bare(go_ref.clone());
        let mut go_module = Module::new(go_ref, RepoPath::new("cache/redis").unwrap());
        go_module.manifest = Some(RepoPath::new("cache/redis/go.mod").unwrap());
        let mut go_entry = entry("core", Version::new(2, 0, 0), true, 1);
        go_entry.module = go_key;

        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(0, 1, 1), true, 0), go_entry],
        );

        let go_target =
            FakeReleaseTarget::new().with_tag_scheme(TagScheme::new("cache/redis/v", ""));
        let mut map = super::super::ReleaseTargets::new();
        map.insert(
            (None, EcosystemId::new("rust").unwrap()),
            Box::new(FakeReleaseTarget::new()),
        );
        map.insert((None, EcosystemId::new("go").unwrap()), Box::new(go_target));
        let writer = FakeVcsWriter::new().with_commit_oid("c0ffee");

        release_apply(
            &plan,
            &[module("core"), go_module],
            &map,
            &FakeVcsReader::new(),
            &writer,
            &ReleaseApplyOptions {
                publish: false,
                ..Default::default()
            },
        )
        .expect("mixed-ecosystem release apply");

        let recorded = writer.writes();
        assert_eq!(
            recorded[0],
            VcsWrite::Commit("release: rust/core@0.1.1, cache/redis/v2.0.0".into())
        );
        assert!(
            recorded.iter().any(
                |w| matches!(w, VcsWrite::CreateTag { name, .. } if name == "rust/core@0.1.1")
            ),
            "rust member keeps its crates.io tag grammar"
        );
        assert!(
            recorded.iter().any(
                |w| matches!(w, VcsWrite::CreateTag { name, .. } if name == "cache/redis/v2.0.0")
            ),
            "go member uses its path-based git tag grammar"
        );
    }

    #[test]
    fn restores_worktree_when_commit_fails() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(0, 1, 1), true, 0)],
        );
        let writer = FakeVcsWriter::new().with_commit_failure("commit failed");

        let error = release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", FakeReleaseTarget::new())]),
            &FakeVcsReader::new(),
            &writer,
            &ReleaseApplyOptions::default(),
        )
        .expect_err("commit failure must surface");

        assert!(error.to_string().contains("commit failed"));
        assert_eq!(
            writer.writes(),
            vec![
                VcsWrite::Commit("release: rust/core@0.1.1".into()),
                VcsWrite::RestoreWorktree
            ]
        );
    }

    #[test]
    fn prepare_failure_reports_restore_failure_without_losing_original_error() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(0, 1, 1), true, 0)],
        );
        let writer = FakeVcsWriter::new().with_restore_failure("restore failed");

        let error = release_apply(
            &plan,
            &[module("core")],
            &targets(vec![(
                "core",
                FakeReleaseTarget::new().with_package_failure("package failed"),
            )]),
            &FakeVcsReader::new(),
            &writer,
            &ReleaseApplyOptions::default(),
        )
        .expect_err("prepare and restore failures must surface together");

        let message = error.to_string();
        assert!(message.contains("release prepare failed"));
        assert!(message.contains("package failed"));
        assert!(message.contains("restore failed"));
        assert_eq!(error.code(), ErrorCode::Internal);
        assert!(
            error
                .cause()
                .is_some_and(|cause| cause.to_string().contains("package failed"))
        );
        assert_eq!(writer.writes(), vec![VcsWrite::RestoreWorktree]);
    }

    #[test]
    fn commit_failure_reports_restore_failure_without_losing_original_error() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(0, 1, 1), true, 0)],
        );
        let writer = FakeVcsWriter::new()
            .with_commit_failure("commit failed")
            .with_restore_failure("restore failed");

        let error = release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", FakeReleaseTarget::new())]),
            &FakeVcsReader::new(),
            &writer,
            &ReleaseApplyOptions::default(),
        )
        .expect_err("commit and restore failures must surface together");

        let message = error.to_string();
        assert!(message.contains("release commit failed"));
        assert!(message.contains("commit failed"));
        assert!(message.contains("restore failed"));
        assert_eq!(error.code(), ErrorCode::Internal);
        assert!(
            error
                .cause()
                .is_some_and(|cause| cause.to_string().contains("commit failed"))
        );
        assert_eq!(
            writer.writes(),
            vec![
                VcsWrite::Commit("release: rust/core@0.1.1".into()),
                VcsWrite::RestoreWorktree
            ]
        );
    }

    #[test]
    fn dirty_worktree_is_rejected_without_allow_dirty() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(0, 1, 1), true, 0)],
        );
        let writer = FakeVcsWriter::new();

        let error = release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", FakeReleaseTarget::new())]),
            &dirty(),
            &writer,
            &ReleaseApplyOptions::default(),
        )
        .expect_err("dirty worktree must be rejected");
        assert!(error.to_string().contains("uncommitted change"));
        assert!(
            writer.writes().is_empty(),
            "no writes on a tripped guardrail"
        );
    }

    #[test]
    fn allow_dirty_bypasses_the_clean_tree_guardrail() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(0, 1, 1), true, 0)],
        );
        let writer = FakeVcsWriter::new();

        let stats = release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", FakeReleaseTarget::new())]),
            &dirty(),
            &writer,
            &ReleaseApplyOptions {
                allow_dirty: true,
                ..Default::default()
            },
        )
        .expect("allow-dirty bypasses the guardrail");
        assert_eq!(stats.published_modules, 1);
    }

    #[test]
    fn package_failure_rolls_back_before_commit() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(0, 1, 1), true, 0)],
        );
        let writer = FakeVcsWriter::new();

        let error = release_apply(
            &plan,
            &[module("core")],
            &targets(vec![(
                "core",
                FakeReleaseTarget::new().with_package_failure("boom"),
            )]),
            &FakeVcsReader::new(),
            &writer,
            &ReleaseApplyOptions::default(),
        )
        .expect_err("package failure surfaces");
        assert!(error.to_string().contains("boom"));

        let recorded = writer.writes();
        assert_eq!(recorded, vec![VcsWrite::RestoreWorktree]);
        assert!(!recorded.iter().any(|w| matches!(w, VcsWrite::Commit(_))));
    }

    #[test]
    fn already_published_version_is_pre_skipped() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(0, 1, 1), true, 0)],
        );
        let target = FakeReleaseTarget::new().with_published_versions(vec![Version::new(0, 1, 1)]);

        let stats = release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", target.clone())]),
            &FakeVcsReader::new(),
            &FakeVcsWriter::new(),
            &ReleaseApplyOptions::default(),
        )
        .expect("pre-skip");

        assert_eq!(stats.published_modules, 0);
        assert_eq!(stats.skipped_published_modules, 1);
        assert!(
            !target
                .calls()
                .iter()
                .any(|c| matches!(c, ReleaseCall::Publish(_))),
            "an already-published version must not be re-published"
        );
    }

    #[test]
    fn already_published_outcome_is_resume_safe() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(0, 1, 1), true, 0)],
        );
        let target =
            FakeReleaseTarget::new().with_publish_outcome(PublishOutcome::AlreadyPublished);

        let stats = release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", target)]),
            &FakeVcsReader::new(),
            &FakeVcsWriter::new(),
            &ReleaseApplyOptions::default(),
        )
        .expect("resume-safe already-published");

        assert_eq!(stats.published_modules, 0);
        assert_eq!(stats.skipped_published_modules, 1);
    }

    #[test]
    fn rate_limited_publish_retries_within_budget() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(0, 1, 1), true, 0)],
        );
        let target = FakeReleaseTarget::new().with_publish_outcomes(vec![
            PublishOutcome::RateLimited { retry_after: None },
            PublishOutcome::Published,
        ]);

        let stats = release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", target)]),
            &FakeVcsReader::new(),
            &FakeVcsWriter::new(),
            &ReleaseApplyOptions::default(),
        )
        .expect("retry then publish");

        assert_eq!(stats.published_modules, 1);
        assert_eq!(stats.rate_limited_waits, 1);
    }

    #[test]
    fn rate_limited_publish_surfaces_exhausted_budget() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(0, 1, 1), true, 0)],
        );
        let target = FakeReleaseTarget::new()
            .with_publish_outcome(PublishOutcome::RateLimited { retry_after: None });

        let error = release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", target)]),
            &FakeVcsReader::new(),
            &FakeVcsWriter::new(),
            &ReleaseApplyOptions {
                retry_budget: 2,
                ..Default::default()
            },
        )
        .expect_err("exhausted budget surfaces");
        assert!(error.to_string().contains("rate-limit retry budget"));
    }

    #[test]
    fn entries_without_publish_needed_are_not_published() {
        let plan = ReleasePlan::new(
            BumpPolicy::SemverCascade,
            vec![entry("core", Version::new(0, 1, 1), false, 0)],
        );
        let target = FakeReleaseTarget::new();

        let stats = release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", target.clone())]),
            &FakeVcsReader::new(),
            &FakeVcsWriter::new(),
            &ReleaseApplyOptions::default(),
        )
        .expect("apply without publish");

        assert_eq!(stats.mutated_modules, 1);
        assert_eq!(stats.tagged_modules, 1);
        assert_eq!(stats.published_modules, 0);
        assert!(
            !target
                .calls()
                .iter()
                .any(|c| matches!(c, ReleaseCall::Publish(_)))
        );
    }
}
