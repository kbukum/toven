//! The release APPLY transaction: clean-tree guardrail, manifest mutation,
//! packaging, a single release commit, per-module tagging, optional push, and
//! the bounded publish loop.
//!
//! The transaction has a hard pre-commit/post-commit boundary. Everything before
//! the commit (mutation + packaging) is undoable: any failure restores the
//! working tree and creates no commit or tag. The commit, tags, optional push,
//! and publish loop run after that boundary and are **not** rolled back — a
//! publish failure surfaces as a typed error and the operator resumes, relying
//! on registry idempotency.

use std::collections::BTreeMap;

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_model::{EcosystemId, Module, ModuleRef};
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
    /// Plan only: validate the guardrail and return without mutating anything.
    pub dry_run: bool,
    /// Maximum rate-limit retries per module in the publish loop.
    pub retry_budget: usize,
}

impl Default for ReleaseApplyOptions {
    fn default() -> Self {
        Self {
            allow_dirty: false,
            push: false,
            dry_run: false,
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
/// Returns a typed error when the clean-tree guardrail trips, a module/target is
/// missing, a pre-commit mutation/package fails (after restoring the working
/// tree), a VCS commit/tag/push fails, or the publish loop exhausts its retry
/// budget.
pub fn release_apply(
    plan: &ReleasePlan,
    modules: &[Module],
    targets: &BTreeMap<EcosystemId, Box<dyn ReleaseTarget>>,
    reader: &dyn VcsReader,
    writer: &dyn VcsWriter,
    options: &ReleaseApplyOptions,
) -> AppResult<ReleaseStats> {
    let mut stats = ReleaseStats::new(plan.entries.len());
    if plan.is_empty() {
        return Ok(stats);
    }

    // The guardrail is part of the dry-run contract: validate it, then stop
    // before any mutation when `dry_run` is set.
    guard_clean_tree(reader, options)?;
    if options.dry_run {
        return Ok(stats);
    }

    let module_by_ref: BTreeMap<&ModuleRef, &Module> =
        modules.iter().map(|module| (&module.id, module)).collect();

    // Pre-commit phase (undoable): apply mutations, then package every module.
    let artifacts = match prepare(plan, &module_by_ref, targets, &mut stats) {
        Ok(artifacts) => artifacts,
        Err(error) => {
            writer.restore_worktree()?;
            return Err(error);
        }
    };

    // Post-commit phase (no rollback): commit once, tag, optionally push, publish.
    let message = commit_message(plan);
    let commit = writer.commit(&message)?;
    for entry in &plan.entries {
        if let Some(version) = &entry.planned_version {
            let name = tag::format(&entry.module, version);
            writer.create_tag(&name, commit.as_str(), Some(&message))?;
            stats.tagged_modules += 1;
        }
    }
    if options.push {
        writer.push(&push_refspecs(plan))?;
    }

    let items = publish_items(plan, &module_by_ref, targets, &artifacts)?;
    publish::run(&items, options.retry_budget, &mut stats)?;

    Ok(stats)
}

/// Reject a dirty working tree unless `--allow-dirty` was requested.
fn guard_clean_tree(reader: &dyn VcsReader, options: &ReleaseApplyOptions) -> AppResult<()> {
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
fn prepare(
    plan: &ReleasePlan,
    module_by_ref: &BTreeMap<&ModuleRef, &Module>,
    targets: &BTreeMap<EcosystemId, Box<dyn ReleaseTarget>>,
    stats: &mut ReleaseStats,
) -> AppResult<BTreeMap<ModuleRef, Artifact>> {
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
fn publish_items<'a>(
    plan: &'a ReleasePlan,
    module_by_ref: &BTreeMap<&'a ModuleRef, &'a Module>,
    targets: &'a BTreeMap<EcosystemId, Box<dyn ReleaseTarget>>,
    artifacts: &'a BTreeMap<ModuleRef, Artifact>,
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
    module_by_ref: &BTreeMap<&'a ModuleRef, &'a Module>,
    reference: &ModuleRef,
) -> AppResult<&'a Module> {
    module_by_ref.get(reference).copied().ok_or_else(|| {
        AppError::invalid_input("release.modules", format!("unknown module '{reference}'"))
    })
}

fn target_for<'a>(
    targets: &'a BTreeMap<EcosystemId, Box<dyn ReleaseTarget>>,
    module: &Module,
) -> AppResult<&'a dyn ReleaseTarget> {
    targets
        .get(&module.id.ecosystem)
        .map(Box::as_ref)
        .ok_or_else(|| {
            AppError::invalid_input(
                "release.target",
                format!("module '{}' has no release target", module.id),
            )
        })
}

/// Build the single release commit message from the released module versions.
fn commit_message(plan: &ReleasePlan) -> String {
    let released = plan
        .entries
        .iter()
        .filter_map(|entry| {
            entry
                .planned_version
                .as_ref()
                .map(|version| tag::format(&entry.module, version))
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("release: {released}")
}

/// Refspecs pushed after tagging: the release commit plus every release tag.
fn push_refspecs(plan: &ReleasePlan) -> Vec<String> {
    let mut refspecs = vec!["HEAD".to_string()];
    for entry in &plan.entries {
        if let Some(version) = &entry.planned_version {
            refspecs.push(format!("refs/tags/{}", tag::format(&entry.module, version)));
        }
    }
    refspecs
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use rskit_version::semver::Version;
    use toven_model::{EcosystemId, Module, ModuleRef, RepoPath};
    use toven_ports::{ChangeRecord, ChangeStatus, PublishOutcome, ReleaseMutation, ReleaseTarget};
    use toven_testkit::{FakeReleaseTarget, FakeVcsReader, FakeVcsWriter, ReleaseCall, VcsWrite};

    use super::{ReleaseApplyOptions, release_apply};
    use crate::release::{ChangelogEntry, ReleaseEntry, ReleasePlan, ReleaseStrategyName};

    fn mref(name: &str) -> ModuleRef {
        ModuleRef::new(EcosystemId::new("rust").unwrap(), name).unwrap()
    }

    fn module(name: &str) -> Module {
        let mut module = Module::new(mref(name), RepoPath::new(format!("crates/{name}")).unwrap());
        module.manifest = Some(RepoPath::new(format!("crates/{name}/Cargo.toml")).unwrap());
        module
    }

    fn entry(name: &str, version: Version, publish_needed: bool, rank: usize) -> ReleaseEntry {
        ReleaseEntry {
            module: mref(name),
            current_version: Version::new(0, 1, 0),
            planned_version: Some(version.clone()),
            mutation: ReleaseMutation::version(version),
            publish_needed,
            topo_rank: rank,
            baseline: None,
            changelog: ChangelogEntry::new(mref(name), "changed", Vec::new()),
        }
    }

    fn targets(
        pairs: Vec<(&str, FakeReleaseTarget)>,
    ) -> BTreeMap<EcosystemId, Box<dyn ReleaseTarget>> {
        // All fixtures use a single `rust` ecosystem; the first target wins.
        let mut map: BTreeMap<EcosystemId, Box<dyn ReleaseTarget>> = BTreeMap::new();
        let (_, target) = pairs.into_iter().next().expect("at least one target");
        map.insert(EcosystemId::new("rust").unwrap(), Box::new(target));
        map
    }

    fn dirty() -> FakeVcsReader {
        FakeVcsReader::new()
            .with_worktree_status(vec![ChangeRecord::new("a.rs", ChangeStatus::Modified)])
    }

    #[test]
    fn applies_mutations_commits_tags_and_publishes_in_order() {
        let plan = ReleasePlan::new(
            ReleaseStrategyName::SemverCascade,
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
            ReleaseStrategyName::SemverCascade,
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
    fn dirty_worktree_is_rejected_without_allow_dirty() {
        let plan = ReleasePlan::new(
            ReleaseStrategyName::SemverCascade,
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
            ReleaseStrategyName::SemverCascade,
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
    fn dry_run_validates_the_guardrail_but_makes_no_changes() {
        let plan = ReleasePlan::new(
            ReleaseStrategyName::SemverCascade,
            vec![entry("core", Version::new(0, 1, 1), true, 0)],
        );
        let target = FakeReleaseTarget::new();
        let writer = FakeVcsWriter::new();

        let stats = release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", target.clone())]),
            &FakeVcsReader::new(),
            &writer,
            &ReleaseApplyOptions {
                dry_run: true,
                ..Default::default()
            },
        )
        .expect("dry run");

        assert_eq!(stats.mutated_modules, 0);
        assert!(writer.writes().is_empty());
        assert!(target.calls().is_empty());
    }

    #[test]
    fn dry_run_still_rejects_a_dirty_worktree() {
        let plan = ReleasePlan::new(
            ReleaseStrategyName::SemverCascade,
            vec![entry("core", Version::new(0, 1, 1), true, 0)],
        );
        let writer = FakeVcsWriter::new();

        let error = release_apply(
            &plan,
            &[module("core")],
            &targets(vec![("core", FakeReleaseTarget::new())]),
            &dirty(),
            &writer,
            &ReleaseApplyOptions {
                dry_run: true,
                ..Default::default()
            },
        )
        .expect_err("dry run validates the guardrail");
        assert!(error.to_string().contains("uncommitted change"));
        assert!(writer.writes().is_empty());
    }

    #[test]
    fn package_failure_rolls_back_before_commit() {
        let plan = ReleasePlan::new(
            ReleaseStrategyName::SemverCascade,
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
            ReleaseStrategyName::SemverCascade,
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
            ReleaseStrategyName::SemverCascade,
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
            ReleaseStrategyName::SemverCascade,
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
            ReleaseStrategyName::SemverCascade,
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
            ReleaseStrategyName::SemverCascade,
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
